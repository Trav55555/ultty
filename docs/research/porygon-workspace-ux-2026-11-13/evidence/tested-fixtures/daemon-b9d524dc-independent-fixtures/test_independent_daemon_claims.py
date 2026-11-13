"""Independent interleaving probes over final routes and real SQLite claims."""
import asyncio

import pytest
import compose_support  # installs the isolated project test bootstrap
from test_approval_version_claim import approval_api, record
from porygon.server.routes import approvals

pytest_plugins = ["conftest"]


@pytest.mark.asyncio
async def test_replacement_after_validation_before_claim_never_executes(approval_api, monkeypatch):
    client, store, calls, _, _ = approval_api
    await store.add_pending_approval(record("human-reviewed-old"), scope="tool_call")
    old = await store.get_pending_approval("same-id")

    async def replace_during_validation(**kwargs):
        await store.add_pending_approval(record("replacement-must-survive"), scope="tool_call")
        return None

    monkeypatch.setattr(approvals, "_ensure_policy_binding_for_approval", replace_during_validation)
    response = await client.post("/approvals/pending/same-id/approve", json={"expected_version": old["decision_version"]})
    assert response.json()["status"] == "stale"
    assert calls == []
    replacement = await store.get_pending_approval("same-id")
    assert replacement["decision_version"] != old["decision_version"]
    assert replacement["context"]["params"]["content"] == "replacement-must-survive"


@pytest.mark.asyncio
async def test_deny_wins_while_approve_waits_before_claim(approval_api, monkeypatch):
    client, store, calls, _, _ = approval_api
    await store.add_pending_approval(record("only-one-decision"), scope="tool_call")
    pending = await store.get_pending_approval("same-id")
    validating = asyncio.Event()
    release_validation = asyncio.Event()

    async def pause_validation(**kwargs):
        validating.set()
        await release_validation.wait()
        return None

    monkeypatch.setattr(approvals, "_ensure_policy_binding_for_approval", pause_validation)
    body = {"expected_version": pending["decision_version"]}
    approving = asyncio.create_task(client.post("/approvals/pending/same-id/approve", json=body))
    await asyncio.wait_for(validating.wait(), 2)
    try:
        denied = await client.post("/approvals/pending/same-id/deny", json=body)
        assert denied.json()["status"] == "denied"
    finally:
        release_validation.set()
    assert (await approving).json()["status"] == "stale"
    assert calls == []
    assert await store.get_pending_approval("same-id") is None


@pytest.mark.asyncio
async def test_claimed_approve_rejects_competing_deny(approval_api):
    client, store, calls, started, release = approval_api
    await store.add_pending_approval(record("claim-before-execution-finishes"), scope="tool_call")
    pending = await store.get_pending_approval("same-id")
    body = {"expected_version": pending["decision_version"]}
    approving = asyncio.create_task(client.post("/approvals/pending/same-id/approve", json=body))
    await asyncio.wait_for(started.wait(), 2)
    try:
        denied = await client.post("/approvals/pending/same-id/deny", json=body)
        assert denied.json()["status"] == "stale"
        assert len(calls) == 1
    finally:
        release.set()
    assert (await approving).json()["status"] == "approved"


@pytest.mark.asyncio
async def test_versioned_manual_scope_never_falls_back_to_heartbeat(approval_api):
    client, store, calls, _, _ = approval_api
    item = record("manual-scope")
    item["scope"] = "manual"
    await store.add_pending_approval(item, scope="manual")
    pending = await store.get_pending_approval("same-id")
    response = await client.post("/approvals/pending/same-id/approve", json={"expected_version": pending["decision_version"]})
    assert response.json()["status"] == "stale"
    assert await store.get_pending_approval("same-id") is not None
    assert calls == []
