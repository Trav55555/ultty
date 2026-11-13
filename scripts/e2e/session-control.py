#!/usr/bin/env python3
"""Headless control/CLI/MCP automation against a real persistent owner.

Set ULTTY_SESSION, ULTTYCTL, COUNTER_APP, ULTTUI_LUA, CARGO_TARGET_DIR and TMPDIR
to a frozen build and task-local directories. No GPU, SSH or external network is
used. The artifact directory must be fresh; failed receipts remain available.
"""

import argparse
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import shutil
import signal
import subprocess
import sys
import time


HERE = Path(__file__).resolve().parent


def load(name, path):
    spec = importlib.util.spec_from_file_location(name, path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


CONTROL = load("persistent_control_gate", HERE / "control-workflows.py")
ATTACH = load("persistent_attach_gate", HERE / "session-host.py")


def save(path, value):
    path.write_text(json.dumps(value, indent=2) + "\n")


def digest(path):
    with open(path, "rb") as source:
        return hashlib.file_digest(source, "sha256").hexdigest()


def short_runtime(target):
    for index in range(100):
        path = target.parent / f"c{index}"
        if len(os.fsencode(path / ("ultty-native-" + "a" * 16) / "ultty-9999999-tab0.sock")) >= 108:
            raise RuntimeError("build directory parent is too long for native socket fixtures")
        try:
            path.mkdir(mode=0o700)
            return path
        except FileExistsError:
            continue
    raise RuntimeError("no short private runtime directory is free")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("artifacts", type=Path)
    args = parser.parse_args()
    out = args.artifacts.resolve()
    out.mkdir(parents=True, exist_ok=True)
    if any(out.iterdir()):
        raise RuntimeError("use a fresh artifact directory")
    binaries = {key: Path(os.environ[key]).resolve()
                for key in ("ULTTY_SESSION", "ULTTYCTL", "COUNTER_APP", "ULTTUI_LUA")}
    repo = HERE.parents[1]
    integration = repo / "dist/shell-integration/ultty.bash"
    lua_fixture = repo / "examples/lua/hello.luau"
    hashes = {str(path): digest(path) for path in [*binaries.values(), integration, lua_fixture]}
    for name in ("session-control.py", "control-workflows.py", "session-host.py"):
        shutil.copy2(HERE / name, out / name)
        hashes[str(HERE / name)] = digest(HERE / name)
    save(out / "inputs.json", {"sha256": hashes, "budget_seconds": 90})
    config = out / "config"
    (config / "ultty").mkdir(parents=True)
    bashrc = out / "bashrc"
    bashrc.write_text("PS1='PERSISTENT> '\nHISTFILE=/dev/null\nsource " + repr(str(integration)) + "\n")
    runtime = short_runtime(Path(os.environ["CARGO_TARGET_DIR"]).resolve())
    name = f"control-gate-{os.getpid()}"
    env = dict(os.environ, XDG_RUNTIME_DIR=str(runtime), XDG_CONFIG_HOME=str(config),
               SHELL="/bin/bash", RUST_LOG="info")
    for key in ("ULTTY_PID", "ULTTY_SOCKET", "ULTTY_AUTH_TOKEN", "ULTTY_PANE"):
        env.pop(key, None)
    streams = []
    attachment = host = None
    started = time.monotonic()
    receipt_file = open(out / "receipts.jsonl", "w")
    host_log = open(out / "host.log", "wb")
    try:
        host = subprocess.Popen([str(binaries["ULTTY_SESSION"]), "host", name, "-e", "/bin/bash",
                                 "--noprofile", "--rcfile", str(bashrc), "-i"], env=env,
                                stdout=host_log, stderr=subprocess.STDOUT)
        control_endpoint = runtime / f"ultty-ctl-{host.pid}.sock"
        token_file = runtime / f"ultty-auth-{host.pid}.token"
        attach_endpoint = runtime / f"ultty-session-{name}.sock"
        admin_file = attach_endpoint.with_suffix(".admin")
        CONTROL.eventually(lambda: control_endpoint.exists() and token_file.is_file() and admin_file.is_file(),
                           "persistent control startup", 20)
        ctl = CONTROL.Control(control_endpoint, token_file.read_text().strip(), receipt_file)
        description = CONTROL.data(ctl.request({"type": "describe"}, owner=False), "description")
        ctl.owner = description["owner"]
        assert ctl.owner["pid"] == host.pid, description
        assert {"new_tab", "wait_job", "run_command_and_wait", "invoke_action", "subscribe_v2"}.issubset(description["supported_operations"]), description
        attachment = ATTACH.Client(attach_endpoint, admin_file.read_text().strip(), name)
        assert attachment.owner == ctl.owner
        assert attachment.snapshot()["owner"] == ctl.owner
        cli_env = dict(env, ULTTY_PID=str(host.pid))
        cli = subprocess.run([str(binaries["ULTTYCTL"]), "--json", "describe"], env=cli_env,
                             capture_output=True, text=True, timeout=5)
        assert cli.returncode == 0, cli.stderr
        cli_description = json.loads(cli.stdout)
        assert cli_description["owner"] == ctl.owner
        save(out / "description.json", cli_description)
        panes = lambda: CONTROL.data(ctl.request({"type": "list_panes"}), "panes")["panes"]
        initial = panes()[0]["id"]
        def prompt():
            text = ctl.request({"type": "get_text", "include_scrollback": False,
                                "scrollback_lines": None, "target": initial}).get("text", "")
            return text.rstrip().endswith("PERSISTENT>")
        CONTROL.eventually(prompt, "integrated persistent bash prompt")
        stale = dict(ctl.owner, epoch="0" * 64 if ctl.owner["epoch"] != "0" * 64 else "1" * 64)
        before = {pane["id"] for pane in panes()}
        CONTROL.error(ctl.request({"type": "with_owner", "owner": stale, "request": {
            "type": "new_tab", "title": None, "command": "/bin/sleep", "args": ["10"],
            "working_dir": None}}, owner=False), 409)
        assert {pane["id"] for pane in panes()} == before
        arguments = ["two words", "$(not-a-shell)", "quoted'word", "line\nbreak"]
        argv_file = out / "argv.json"
        code = "import json,os,pathlib,sys;pathlib.Path(sys.argv[1]).write_text(json.dumps({'args':sys.argv[2:],'cwd':os.getcwd()}));raise SystemExit(7)"
        fast = CONTROL.data(ctl.request({"type": "new_tab", "title": None, "command": sys.executable,
                                        "args": ["-c", code, str(argv_file), *arguments],
                                        "working_dir": str(out)}), "tab_opened")["pane"]
        CONTROL.eventually(lambda: argv_file.is_file(), "exact argv child output")
        assert json.loads(argv_file.read_text()) == {"args": arguments, "cwd": str(out)}
        def exited():
            result = ctl.request({"type": "job_status", "job": fast})
            return result if result.get("outcome", {}).get("state") == "exited" else None
        done = CONTROL.eventually(exited, "retained immediate child completion")
        assert done["outcome"] == {"state": "exited", "status": 7}
        waited = CONTROL.data(ctl.request({"type": "wait_job", "job": fast, "timeout_ms": 100}), "process_job")
        assert waited["outcome"] == done["outcome"]
        cli_wait = subprocess.run([str(binaries["ULTTYCTL"]), "wait-job", str(fast), "--timeout", "100"],
                                  env=cli_env, capture_output=True, text=True, timeout=5)
        assert cli_wait.returncode == 7, cli_wait
        CONTROL.eventually(prompt, "bash ready after child cleanup")
        def command(text):
            CONTROL.eventually(prompt, "empty integrated bash prompt")
            return CONTROL.data(ctl.request({"type": "run_command_and_wait", "command": text,
                                             "timeout_ms": 5000, "target": initial}), "shell_command_finished")
        first = command("false")
        assert first["exit_code"] == 1 and first["sequence"] > 0
        stream, subscribed = ctl.subscribe({"type": "subscribe_v2", "cursor": None, "target": initial})
        streams.append(stream)
        CONTROL.data(subscribed, "subscribed_v2")
        assert subscribed["snapshot"] and subscribed["replayed"] == 0
        reported = command("printf '\\033]7501;state=done:id=agent:app=session-control:msg=RG9uZQ==\\007'")
        assert reported["sequence"] > first["sequence"] and reported["exit_code"] == 0
        status = CONTROL.data(ctl.request({"type": "program_status", "target": initial}), "program_status")
        assert any(record["id"] == "agent" and record["msg"] == "Done" for record in status["records"])
        cursor = None
        for _ in range(20):
            event = stream.next()
            if event["event"]["event"] == "command_finished":
                cursor = event["cursor"]
                break
        assert cursor is not None
        stream.close()
        command("true")
        replay, response = ctl.subscribe({"type": "subscribe_v2", "cursor": cursor, "target": initial})
        streams.append(replay)
        CONTROL.data(response, "subscribed_v2")
        assert response["snapshot"] is None and response["replayed"] > 0
        events = [replay.next() for _ in range(response["replayed"])]
        assert any(event["event"]["event"] == "command_finished" for event in events)
        assert all(event["cursor"]["sequence"] > cursor["sequence"] for event in events)
        resync, response = ctl.subscribe({"type": "subscribe_v2", "cursor": dict(cursor, owner=stale), "target": None})
        streams.append(resync)
        CONTROL.data(response, "resync_required")
        assert response["reason"] == "owner_changed"

        def scene_with(pane, text):
            scenes = ctl.request({"type": "regions", "target": pane}).get("scenes", [])
            return next((scene for scene in scenes if any(region.get("text") == text for region in scene["regions"])), None)

        def activate(pane, expected, wanted):
            for attempt in range(3):
                scene = CONTROL.eventually(lambda: scene_with(pane, expected), "native committed model")
                # Startup may still reconcile layout/focus after publishing
                # its first scene. Give those messages a turn before pinning.
                time.sleep(0.1)
                scene = CONTROL.eventually(lambda: scene_with(pane, expected), "native settled model")
                region = next(region for region in scene["regions"] if region.get("metadata", {}).get("ulttui.widget_id") == "inc")
                assert "activate" in region["metadata"]["ulttui.actions"].split(",")
                result = ctl.request({"type": "invoke_action", "target": pane,
                                      "session": scene["session"], "expected_generation": scene["shown_generation"],
                                      "region": region["id"], "widget_id": "inc", "action": "activate"})
                # This exact error is returned before send_message in
                # NativeSession::request_semantic_action. Reinspect only a
                # definitive non-dispatch; never retry unknown/applied results.
                if result == {"status": "error", "code": 15, "message":
                              "Semantic action dispatch failed: stale or uncommitted scene; reinspect before retrying"} and attempt < 2:
                    continue
                CONTROL.data(result, "action_applied")
                break
            assert result["session"] == scene["session"]
            CONTROL.eventually(lambda: scene_with(pane, wanted), "application-owned activation update")

        def native(program, arguments, expected, wanted):
            pane = CONTROL.data(ctl.request({"type": "new_tab", "title": None, "command": str(program),
                                             "args": arguments, "working_dir": None}), "tab_opened")["pane"]
            activate(pane, expected, wanted)
            return pane
        counter = native(binaries["COUNTER_APP"], [], "0", "1")
        lua = native(binaries["ULTTUI_LUA"], [str(lua_fixture)], "Count: 0", "Count: 1")
        # Local privileged automation operates on active logical panes even
        # when no attachment reader or GPU viewer exists.
        attachment.sock.close()
        attachment = None
        time.sleep(0.1)
        for pane, expected, wanted in [(counter, "1", "2"), (lua, "Count: 1", "Count: 2")]:
            assert ctl.request({"type": "focus_pane", "target": pane})["status"] == "ok"
            activate(pane, expected, wanted)
        save(out / "detached-actions.json", {"attachment_readers": 0, "counter": 2, "lua": 2})
        requests = [
            {"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {"protocolVersion": "2024-11-05", "capabilities": {}}},
            {"jsonrpc": "2.0", "id": 2, "method": "ping"},
            {"jsonrpc": "2.0", "id": 3, "method": "tools/list"},
            {"jsonrpc": "2.0", "id": 4, "method": "tools/call", "params": {"name": "ultty_list_panes", "arguments": {}}},
        ]
        mcp = subprocess.run([str(binaries["ULTTYCTL"]), "mcp"], env=cli_env,
                             input="".join(json.dumps(request) + "\n" for request in requests),
                             capture_output=True, text=True, timeout=10)
        assert mcp.returncode == 0, mcp.stderr
        responses = [json.loads(line) for line in mcp.stdout.splitlines()]
        assert [response["id"] for response in responses] == [1, 2, 3, 4]
        assert responses[1]["result"] == {}
        tools = {tool["name"] for tool in responses[2]["result"]["tools"]}
        assert "ultty_list_panes" in tools and "ultty_screenshot" not in tools
        assert not responses[3]["result"]["isError"], responses[3]
        mcp_panes = json.loads(responses[3]["result"]["content"][0]["text"])
        assert {pane["id"] for pane in mcp_panes} == {pane["id"] for pane in panes()}
        save(out / "mcp.json", responses)
        assert host.poll() is None
        attachment = ATTACH.Client(attach_endpoint, admin_file.read_text().strip(), name)
        assert attachment.owner == ctl.owner
        assert all(digest(Path(path)) == expected for path, expected in hashes.items())
        stopped = attachment.mutate({"type": "terminate"})
        assert stopped.get("type") == "ok", stopped
        host.wait(timeout=5)
        assert host.returncode == 0
        attachment.sock.close()
        attachment = None
        save(out / "result.json", {"passed": True, "seconds": time.monotonic() - started,
                                  "owner": ctl.owner, "shell": initial, "retained_job": fast,
                                  "counter": counter, "lua": lua, "sha256": hashes})
        print(f"PASS persistent control owner={host.pid} argv/job/shell/actions/replay/MCP")
    finally:
        for stream in streams:
            stream.close()
        if attachment is not None and host is not None and host.poll() is None:
            result = attachment.mutate({"type": "terminate"})
            if result.get("type") == "ok":
                host.wait(timeout=5)
            attachment.sock.close()
        CONTROL.terminate(host)
        receipt_file.close()
        host_log.close()
        shutil.rmtree(runtime)


if __name__ == "__main__":
    def expired(_signal, _frame):
        raise TimeoutError("persistent control gate exceeded its fixed 90 second budget")
    signal.signal(signal.SIGALRM, expired)
    signal.alarm(90)
    try:
        main()
    finally:
        signal.alarm(0)
