#!/usr/bin/env python3
"""Real two-owner PID selection, MCP pinning and split-target regression gate.

Build first; pass frozen ultty-session and ulttyctl binaries. This gate starts
only private disposable hosts, requires a fresh output directory, and records
public identities/responses without auth tokens or authentication envelopes.
Fixed budget: 60 seconds, plus 15 seconds for owned-process cleanup.
"""

import argparse
import hashlib
import json
import os
from pathlib import Path
import select
import signal
import socket
import subprocess
import tempfile
import time
import traceback


def digest(path):
    with open(path, "rb") as source:
        return hashlib.file_digest(source, "sha256").hexdigest()


def eventually(action, description, timeout=5):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        value = action()
        if value:
            return value
        time.sleep(0.02)
    raise AssertionError(f"timed out: {description}")


def run(args):
    out = args.output.resolve()
    out.mkdir(parents=True, exist_ok=False, mode=0o700)
    (out / "control-pid.py").write_bytes(Path(__file__).read_bytes())
    binaries = {"host": args.session_bin.resolve(), "ctl": args.ctl_bin.resolve()}
    (out / "inputs.json").write_text(json.dumps({"budget_seconds": 60,
        "sha256": {str(path): digest(path) for path in [*binaries.values(), Path(__file__).resolve()]}}, indent=2) + "\n")
    started = time.monotonic()
    hosts, mcp_processes, files = [], [], []
    originals = {}
    result = {"passed": False}
    receipts = (out / "receipts.jsonl").open("w")
    runtime_parent = Path(os.environ.get("TMPDIR", "/tmp")).resolve().parent
    runtime = tempfile.TemporaryDirectory(prefix="cp-", dir=runtime_parent)
    env = dict(os.environ, XDG_RUNTIME_DIR=runtime.name)
    for key in ("ULTTY_PID", "ULTTY_PANE", "ULTTY_SOCKET", "ULTTY_AUTH_TOKEN", "ULTTY_OWNER_EPOCH"):
        env.pop(key, None)

    def receipt(event, **fields):
        receipts.write(json.dumps({"event": event, **fields}) + "\n")
        receipts.flush()

    def ctl(*arguments, extra=None, check=True):
        actual_env = dict(env, **(extra or {}))
        completed = subprocess.run([str(binaries["ctl"]), *arguments], env=actual_env,
                                   capture_output=True, text=True, timeout=8)
        receipt("cli", argv=list(arguments), selected_env={key: value for key, value in (extra or {}).items()
                if key in ("ULTTY_PID", "ULTTY_PANE")}, exit_code=completed.returncode,
                stdout=completed.stdout[:16384], stderr=completed.stderr[:8192])
        if check:
            assert completed.returncode == 0, "CLI refused; see receipt"
        return completed

    def admin(host, operation):
        return subprocess.run([str(binaries["host"]), operation, host["name"]], env=env,
                              capture_output=True, text=True, timeout=5)

    def launch(label):
        name = f"pid-{os.getpid()}-{label}"
        log = (out / (label + ".log")).open("wb")
        files.append(log)
        process = subprocess.Popen([str(binaries["host"]), "host", name, "-e", "/bin/sleep", "60"],
                                   env=env, stdout=log, stderr=log)
        host = {"name": name, "process": process, "pid": process.pid}
        hosts.append(host)
        host["endpoint"] = Path(runtime.name) / f"ultty-ctl-{process.pid}.sock"
        host["token"] = Path(runtime.name) / f"ultty-auth-{process.pid}.token"
        credential = Path(runtime.name) / f"ultty-session-{name}.admin"
        eventually(lambda: credential.is_file() and host["token"].is_file(), "host startup")
        description = json.loads(ctl("--pid", str(process.pid), "--json", "describe").stdout)
        host["owner"] = description["owner"]
        assert host["owner"]["pid"] == process.pid
        snapshot = json.loads(admin(host, "snapshot").stdout)["reply"]["snapshot"]
        host["child"] = snapshot["panes"][0]["pid"]
        receipt("host", name=name, owner=host["owner"], child_pid=host["child"])
        return host

    def mcp(host):
        log = (out / (host["name"] + "-mcp.log")).open("wb")
        files.append(log)
        process = subprocess.Popen([str(binaries["ctl"]), "--pid", str(host["pid"]), "mcp"],
                                   env=dict(env, ULTTY_PID="invalid"), stdin=subprocess.PIPE,
                                   stdout=subprocess.PIPE, stderr=log, text=True, bufsize=1)
        mcp_processes.append(process)
        return process

    def rpc(process, identity, method, params=None):
        message = {"jsonrpc": "2.0", "id": identity, "method": method, "params": params or {}}
        process.stdin.write(json.dumps(message) + "\n")
        process.stdin.flush()
        ready, _, _ = select.select([process.stdout], [], [], 8)
        assert ready, "MCP response deadline elapsed"
        response = json.loads(process.stdout.readline())
        assert response["id"] == identity
        receipt("mcp", request=message, response=response)
        return response

    def panes(process, identity):
        response = rpc(process, identity, "tools/call", {"name": "ultty_list_panes", "arguments": {}})
        assert not response["result"]["isError"], response
        return json.loads(response["result"]["content"][0]["text"])

    try:
        first = launch("first")
        time.sleep(0.05)
        second = launch("second")
        assert json.loads(ctl("--json", "describe").stdout)["owner"] == second["owner"]
        for invalid in ("", "0", "invalid", "-1"):
            assert ctl("--json", "describe", extra={"ULTTY_PID": invalid}, check=False).returncode != 0
            selected = ctl("--pid", str(first["pid"]), "--json", "describe", extra={"ULTTY_PID": invalid})
            assert json.loads(selected.stdout)["owner"] == first["owner"]
        assert ctl("mcp", "--pid", str(first["pid"]), extra={"ULTTY_PID": str(second["pid"])}, check=False).returncode == 2

        first_mcp = mcp(first)
        initialized = rpc(first_mcp, 1, "initialize", {"protocolVersion": "2025-06-18"})
        assert initialized["result"]["serverInfo"]["name"] == "ultty"
        tools = rpc(first_mcp, 2, "tools/list")["result"]["tools"]
        assert any(tool["name"] == "ultty_invoke_action" for tool in tools)
        assert not any(tool["name"] == "ultty_screenshot" for tool in tools)
        assert [pane["pid"] for pane in panes(first_mcp, 3)] == [first["child"]]
        second_mcp = mcp(second)
        assert [pane["pid"] for pane in panes(second_mcp, 1)] == [second["child"]]

        # An explicit owner must not inherit another owner's implicit pane ID.
        split = json.loads(ctl("--pid", str(second["pid"]), "--json", "split", "right", "/bin/sleep", "30",
                               extra={"ULTTY_PID": str(first["pid"]), "ULTTY_PANE": "999"}).stdout)
        assert split["type"] == "pane_opened"
        assert len(json.loads(ctl("--pid", str(first["pid"]), "--json", "panes").stdout)["panes"]) == 1
        assert len(json.loads(ctl("--pid", str(second["pid"]), "--json", "panes").stdout)["panes"]) == 2
        explicit = ctl("--pid", str(second["pid"]), "--pane", "999", "split", "right", check=False)
        assert explicit.returncode != 0

        # Rotate only this disposable host's local control credential. New CLI
        # discovery reads it; the existing MCP must keep its original token.
        originals[second["token"]] = second["token"].read_text()
        second["token"].write_text("0" * 64 + "\n")
        denied = rpc(second_mcp, 2, "tools/call", {"name": "ultty_list_panes", "arguments": {}})
        assert denied["result"]["isError"]
        assert json.loads(ctl("--pid", str(second["pid"]), "--json", "describe").stdout)["owner"] == second["owner"]
        second["token"].write_text(originals.pop(second["token"]))

        assert admin(first, "stop").returncode == 0
        first["process"].wait(timeout=5)
        refused = rpc(first_mcp, 4, "tools/call", {"name": "ultty_list_panes", "arguments": {}})
        assert refused["result"]["isError"]
        assert ctl("--json", "describe", extra={"ULTTY_PID": str(first["pid"])}, check=False).returncode != 0
        assert ctl("--pid", str(first["pid"]), "--json", "describe", extra={"ULTTY_PID": str(second["pid"])}, check=False).returncode != 0
        assert json.loads(ctl("--pid", str(second["pid"]), "--json", "describe", extra={"ULTTY_PID": str(first["pid"])}).stdout)["owner"] == second["owner"]
        result.update(passed=True, owners=[first["owner"], second["owner"]],
                      assertions=["strict env selection", "explicit PID precedence", "MCP owner pin",
                                  "MCP token pin", "MCP capabilities", "foreign implicit pane refused",
                                  "unavailable selected owner never falls back"])
    except Exception:
        (out / "failure.log").write_text(traceback.format_exc())
        raise
    finally:
        signal.alarm(15)
        try:
            for path, token in originals.items():
                if path.exists():
                    path.write_text(token)
            for process in mcp_processes:
                if process.poll() is None:
                    process.terminate()
                process.wait(timeout=3)
            for host in hosts:
                if host["process"].poll() is None:
                    admin(host, "stop")
                    host["process"].wait(timeout=5)
            result["cleanup"] = "owned hosts and MCP processes stopped"
        except Exception:
            result["passed"] = False
            result["cleanup"] = "failed; inspect owned fixture processes"
            raise
        finally:
            for file in files:
                file.close()
            runtime.cleanup()
            receipts.close()
            result["seconds"] = time.monotonic() - started
            (out / "result.json").write_text(json.dumps(result, indent=2) + "\n")
    print(f"control-pid: passed ({out})")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--session-bin", type=Path, required=True)
    parser.add_argument("--ctl-bin", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    def expired(_signal, _frame):
        raise TimeoutError("control-pid exceeded bounded deadline")
    signal.signal(signal.SIGALRM, expired)
    signal.alarm(60)
    try:
        run(args)
    finally:
        signal.alarm(0)


if __name__ == "__main__":
    main()
