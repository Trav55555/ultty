#!/usr/bin/env python3
"""Bounded App-dispatch gate for control schema 2, using a fresh Xvfb owner.

Build binaries first. Run with --ultty PATH --ulttyctl PATH --output FRESH_DIR.
Leaves request receipts, shell fixture and logs; never accesses existing windows.
"""

import argparse
import hashlib
import json
import os
from pathlib import Path
import select
import shutil
import signal
import socket
import subprocess
import tempfile
import time
import traceback


class EventStream:
    def __init__(self, connection):
        self.connection = connection
        self.pending = b""

    def next(self, timeout=5):
        deadline = time.monotonic() + timeout
        while b"\n" not in self.pending:
            left = deadline - time.monotonic()
            assert left > 0, "event deadline expired"
            ready, _, _ = select.select([self.connection], [], [], left)
            assert ready, "event deadline expired"
            chunk = self.connection.recv(65536)
            assert chunk, "event stream closed before expected record"
            self.pending += chunk
            assert len(self.pending) <= 2 * 1024 * 1024, "oversized event line"
        line, self.pending = self.pending.split(b"\n", 1)
        return json.loads(line)

    def close(self):
        self.connection.close()


class Control:
    def __init__(self, endpoint, token, receipt):
        self.endpoint = endpoint
        self.token = token
        self.owner = None
        self.receipt = receipt

    def connect(self, request, owner=True):
        if owner:
            request = {"type": "with_owner", "owner": self.owner, "request": request}
        envelope = {"output_format": "json", "request": {
            "type": "with_token", "token": self.token, "request": request,
        }}
        connection = socket.socket(socket.AF_UNIX)
        connection.settimeout(15)
        connection.connect(str(self.endpoint))
        connection.sendall(json.dumps(envelope).encode() + b"\n")
        return connection

    def request(self, request, owner=True):
        with self.connect(request, owner) as connection:
            connection.shutdown(socket.SHUT_WR)
            response = EventStream(connection).next(12)
        brief = dict(request)
        if "text" in brief:
            brief["text"] = {"bytes": len(brief["text"].encode())}
        self.receipt.write(json.dumps({"request": brief, "response": response}) + "\n")
        self.receipt.flush()
        return response

    def subscribe(self, request):
        stream = EventStream(self.connect(request))
        first = stream.next()
        return stream, first


def data(response, kind):
    assert response.get("status") == "data" and response.get("type") == kind, response
    return response


def error(response, code=None):
    assert response.get("status") == "error", response
    if code is not None:
        assert response["code"] == code, response
    return response


def eventually(function, description, timeout=8):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        result = function()
        if result:
            return result
        time.sleep(0.025)
    raise AssertionError(f"deadline expired: {description}")


def file_hash(path):
    return hashlib.sha256(Path(path).read_bytes()).hexdigest()


def terminate(process):
    if process is None or process.poll() is not None:
        return
    process.terminate()
    try:
        process.wait(timeout=5)
    except subprocess.TimeoutExpired:
        process.kill()
        process.wait(timeout=5)


def run(args):
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=False)
    ultty, ulttyctl = args.ultty.resolve(), args.ulttyctl.resolve()
    repo = Path(__file__).resolve().parents[2]
    integration = repo / "dist/shell-integration/ultty.bash"
    fixture = output / "bashrc"
    fixture.write_text("PS1='CONTROL> '\nsource " + repr(str(integration)) + "\n")
    config = output / "config/ultty"
    config.mkdir(parents=True)
    (config / "config.toml").write_text("[behavior]\ncursor_blink = false\n")
    binaries = {str(path): file_hash(path) for path in (ultty, ulttyctl, integration, Path(__file__).resolve())}
    (output / "inputs.json").write_text(json.dumps({"sha256": binaries, "budget_seconds": 90}, indent=2))
    xvfb = terminal = None
    streams = []
    started = time.monotonic()
    with tempfile.TemporaryDirectory(prefix="uc-", dir="/tmp") as runtime_dir, \
            (output / "xvfb.log").open("wb") as xvfb_log, \
            (output / "ultty.log").open("wb") as terminal_log, \
            (output / "receipts.jsonl").open("w") as receipt:
        os.chmod(runtime_dir, 0o700)
        try:
            display_file = output / "display"
            with display_file.open("wb") as display:
                xvfb = subprocess.Popen(["Xvfb", "-displayfd", str(display.fileno()),
                                         "-screen", "0", "1280x800x24", "-nolisten", "tcp"],
                                        pass_fds=(display.fileno(),), stdout=xvfb_log, stderr=xvfb_log)
                number = eventually(lambda: display_file.read_text().strip(), "Xvfb display", 5)
            env = os.environ.copy()
            for key in ("WAYLAND_DISPLAY", "ULTTY_PID", "ULTTY_SOCKET", "ULTTY_AUTH_TOKEN"):
                env.pop(key, None)
            env.update(DISPLAY=f":{number}", XDG_RUNTIME_DIR=runtime_dir,
                       XDG_CONFIG_HOME=str(output / "config"), SHELL="/bin/bash",
                       RUST_LOG="info,wgpu_core=warn,wgpu_hal=warn")
            candidates = list(Path("/usr/share/vulkan/icd.d").glob("lvp_icd*.json"))
            if candidates and "VK_ICD_FILENAMES" not in env:
                env["VK_ICD_FILENAMES"] = str(candidates[0])
            terminal = subprocess.Popen([str(ultty), "-e", "/bin/bash", "--noprofile",
                                         "--rcfile", str(fixture), "-i"], env=env,
                                        stdout=terminal_log, stderr=terminal_log)
            endpoint = Path(runtime_dir) / f"ultty-ctl-{terminal.pid}.sock"
            token_file = Path(runtime_dir) / f"ultty-auth-{terminal.pid}.token"
            eventually(lambda: endpoint.exists() and token_file.exists(), "control startup", 30)
            ctl = Control(endpoint, token_file.read_text().strip(), receipt)
            description = data(ctl.request({"type": "describe"}, owner=False), "description")
            assert description["schema_version"] == 2
            ctl.owner = description["owner"]
            assert ctl.owner["pid"] == terminal.pid and len(ctl.owner["epoch"]) == 64
            assert "subscribe_v2" in description["supported_operations"]
            panes = lambda: data(ctl.request({"type": "list_panes"}), "panes")["panes"]
            initial = panes()[0]["id"]
            eventually(lambda: "CONTROL>" in ctl.request({"type": "get_text", "include_scrollback": False,
                                                          "scrollback_lines": None, "target": initial}).get("text", ""),
                       "integrated bash prompt")

            # A stale epoch must fail before creating a pane or registering a wait.
            before = {pane["id"] for pane in panes()}
            stale = dict(ctl.owner, epoch="0" * 64)
            error(ctl.request({"type": "with_owner", "owner": stale, "request": {
                "type": "new_tab", "title": None, "command": "/bin/sleep", "args": ["10"], "working_dir": None,
            }}), 409)
            assert {pane["id"] for pane in panes()} == before
            for request in ({"type": "new_tab", "title": None, "command": None, "args": ["ignored"], "working_dir": None},
                            {"type": "split_pane", "direction": "right", "command": None, "args": [], "working_dir": "/tmp", "target": initial}):
                error(ctl.request(request), 4)

            # Instant shell completion is identified by its reserved sequence.
            outcome = data(ctl.request({"type": "run_command_and_wait", "command": "false",
                                       "timeout_ms": 5000, "target": initial}), "shell_command_finished")
            assert outcome["pane"] == initial and outcome["sequence"] > 0 and outcome["exit_code"] == 1
            eventually(lambda: "CONTROL>" in ctl.request({"type": "get_text", "include_scrollback": False,
                                                          "scrollback_lines": None, "target": initial}).get("text", ""),
                       "prompt after command")

            # Direct-child wait after completion works even after pane cleanup.
            fast = data(ctl.request({"type": "new_tab", "title": None, "command": "/bin/sh",
                                    "args": ["-c", "exit 7"], "working_dir": None}), "tab_opened")["pane"]
            result = eventually(lambda: (lambda item: item if item.get("outcome", {}).get("state") == "exited" else None)(
                ctl.request({"type": "job_status", "job": fast})), "retained fast child")
            assert result["outcome"]["status"] == 7
            waited = data(ctl.request({"type": "wait_job", "job": fast, "timeout_ms": 100}), "process_job")
            assert waited["outcome"] == {"state": "exited", "status": 7}
            error(ctl.request({"type": "job_status", "job": 2**63}), 404)

            # Program report reaches query, projected pane chrome and only V2 events.
            v2, snapshot = ctl.subscribe({"type": "subscribe_v2", "cursor": None, "target": None})
            streams.append(v2)
            data(snapshot, "subscribed_v2")
            assert snapshot["snapshot"] and snapshot["replayed"] == 0
            legacy, first = ctl.subscribe({"type": "subscribe", "target": None})
            streams.append(legacy)
            data(first, "subscribed")
            status_script = "printf '\\033]7501;state=working:id=agent:app=control-gate:progress=25:msg=V29ya2luZw==\\007\\007'; sleep 30"
            active = data(ctl.request({"type": "new_tab", "title": None, "command": "/bin/sh",
                                      "args": ["-c", status_script], "working_dir": None}), "tab_opened")["pane"]
            status = eventually(lambda: (lambda item: item if item.get("records") else None)(
                ctl.request({"type": "program_status", "target": active})), "program status")
            assert status["records"][0]["msg"] == "Working" and status["records"][0]["progress"] == 25
            projected = eventually(lambda: next((pane for pane in panes() if pane["id"] == active and pane.get("status", {}).get("label") == "Working"), None), "status chrome")
            assert projected["status"]["progress"] == 0.25
            status_event = None
            for _ in range(30):
                record = v2.next()
                if record["event"]["event"] == "program_status_changed" and record["event"]["pane"] == active:
                    status_event = record
                    break
            assert status_event and status_event["event"]["generation"] >= status["generation"]
            for _ in range(30):
                event = legacy.next()
                assert event["event"] != "program_status_changed", event
                if event["event"] == "bell" and event["pane"] == active:
                    break
            else:
                raise AssertionError("legacy stream missed the status fixture bell")
            resume = status_event["cursor"]
            v2.close()
            legacy.close()
            # A new subscriber replays notifications published while disconnected.
            data(ctl.request({"type": "run_command_and_wait", "command": "true", "timeout_ms": 5000,
                              "target": initial}), "shell_command_finished")
            replay, ack = ctl.subscribe({"type": "subscribe_v2", "cursor": resume, "target": initial})
            streams.append(replay)
            data(ack, "subscribed_v2")
            assert ack["snapshot"] is None and ack["replayed"] >= 1
            records = [replay.next() for _ in range(ack["replayed"])]
            assert all(record["cursor"]["sequence"] > resume["sequence"] for record in records)
            assert any(record["event"]["event"] == "command_finished" for record in records)
            bad_cursor = dict(resume, owner=stale)
            resync, response = ctl.subscribe({"type": "subscribe_v2", "cursor": bad_cursor, "target": None})
            streams.append(resync)
            data(response, "resync_required")
            assert response["reason"] == "owner_changed"

            # Focus/zoom/swap preserve identities and saved recipe geometry.
            sibling = data(ctl.request({"type": "split_pane", "direction": "right", "command": "/bin/sh",
                                       "args": ["-c", "stty raw -echo; printf QUEUE_READY; exec sleep 60"], "working_dir": None, "target": initial}), "pane_opened")["pane"]
            assert ctl.request({"type": "focus_pane", "target": initial})["status"] == "ok"
            assert ctl.request({"type": "workspace_save", "name": "before"})["status"] == "ok"
            recipe = json.loads((config / "sessions/before.json").read_text())
            identities = {pane["id"]: pane["pid"] for pane in panes()}
            assert ctl.request({"type": "zoom_pane", "target": sibling, "zoomed": True})["status"] == "ok"
            assert next(pane for pane in panes() if pane["id"] == sibling)["focused"]
            assert ctl.request({"type": "workspace_save", "name": "zoomed"})["status"] == "ok"
            zoomed = json.loads((config / "sessions/zoomed.json").read_text())
            assert [tab["layout"] for tab in recipe["tabs"]] == [tab["layout"] for tab in zoomed["tabs"]]
            assert ctl.request({"type": "zoom_pane", "target": sibling, "zoomed": False})["status"] == "ok"
            assert ctl.request({"type": "swap_panes", "first": initial, "second": sibling})["status"] == "ok"
            assert {pane["id"]: pane["pid"] for pane in panes()} == identities
            for name in ("../escape", "last", "", "bad/name"):
                error(ctl.request({"type": "workspace_save", "name": name}))
            assert data(ctl.request({"type": "workspace_list"}), "workspaces")["names"] == ["before", "zoomed"]
            assert not (output / "config/escape.json").exists()

            # Raw, non-reading child avoids canonical input discard/echo masking backpressure.
            eventually(lambda: "QUEUE_READY" in ctl.request({"type": "get_text", "include_scrollback": False,
                                                              "scrollback_lines": None, "target": sibling}).get("text", ""),
                       "raw non-reading child")
            # Exercise bounded queue acceptance through App, below per-request cap.
            refused = False
            for index in range(300):
                reply = ctl.request({"type": "send_text", "text": "x" * 65536, "target": sibling})
                if reply["status"] == "error":
                    error(reply, 12)
                    refused = True
                    break
                assert reply["status"] == "ok"
            assert refused, "PTY queue did not refuse input within 18.75 MiB budget"

            cli_env = dict(env, ULTTY_PID=str(terminal.pid))
            completed = subprocess.run([str(ulttyctl), "wait-job", str(fast), "--timeout", "100"],
                                       env=cli_env, capture_output=True, text=True, timeout=5)
            assert completed.returncode == 7, completed
            for selected in ("bad-pid", "999999999"):
                completed = subprocess.run([str(ulttyctl), "panes"], env=dict(cli_env, ULTTY_PID=selected),
                                           capture_output=True, text=True, timeout=5)
                assert completed.returncode != 0 and completed.stderr, completed
            elapsed = time.monotonic() - started
            assert elapsed < 90, f"gate exceeded fixed 90 second budget: {elapsed}"
            (output / "result.json").write_text(json.dumps({"passed": True, "seconds": elapsed,
                "owner": ctl.owner, "queue_accepted_requests": index, "sha256": binaries}, indent=2))
            print(f"control-workflows: passed ({output})")
        finally:
            for stream in streams:
                stream.close()
            terminate(terminal)
            terminate(xvfb)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--ultty", required=True, type=Path)
    parser.add_argument("--ulttyctl", required=True, type=Path)
    parser.add_argument("--output", required=True, type=Path)
    args = parser.parse_args()
    assert shutil.which("Xvfb"), "Xvfb is required"
    def deadline_expired(_signum, _frame):
        raise TimeoutError("control-workflows exceeded fixed 90 second wall budget")

    signal.signal(signal.SIGALRM, deadline_expired)
    signal.alarm(90)
    try:
        run(args)
    except Exception:
        if args.output.exists():
            (args.output / "failure.log").write_text(traceback.format_exc())
        raise
    finally:
        signal.alarm(0)


if __name__ == "__main__":
    main()
