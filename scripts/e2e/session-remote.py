#!/usr/bin/env python3
"""Fixed 60s local gate: actual SSH stdio helper through a fake ssh executable.

No SSH server, network connection or Tailscale enrollment is used. Binaries must
already be built. All artifacts and scoped credentials use a fresh output dir.
"""

import argparse
import hashlib
import json
import os
from pathlib import Path
import signal
import socket
import struct
import subprocess
import sys
import tempfile
import time
import traceback


def eventually(function, timeout=5):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        value = function()
        if value:
            return value
        time.sleep(0.02)
    raise AssertionError("bounded condition did not become true")


def frame(value):
    body = json.dumps(value, separators=(",", ":")).encode()
    assert len(body) <= 1024 * 1024
    return b"UAT1" + struct.pack("<I", len(body)) + body


def read_exact(stream, count):
    result = bytearray()
    while len(result) < count:
        chunk = stream.recv(min(65536, count - len(result)))
        assert chunk, "attachment closed before complete frame"
        result += chunk
    return result


def response(stream):
    header = read_exact(stream, 8)
    assert header[:4] == b"UAT1"
    length = struct.unpack("<I", header[4:])[0]
    assert 0 < length <= 16 * 1024 * 1024
    return json.loads(read_exact(stream, length))


def run(args):
    out = args.output.resolve()
    out.mkdir(parents=True, exist_ok=False)
    binary = args.session_bin.resolve()
    sources = (binary, Path(__file__).resolve())
    hashes = {str(path): hashlib.sha256(path.read_bytes()).hexdigest() for path in sources}
    (out / "inputs.json").write_text(json.dumps({"sha256": hashes, "budget_seconds": 60}, indent=2))
    env = os.environ.copy()
    base = Path(env.get("TMPDIR", "/tmp"))
    host = helper_client = None
    sockets = []
    started = time.monotonic()
    with tempfile.TemporaryDirectory(prefix="ur-", dir=base) as runtime, \
            (out / "host.log").open("wb") as log, \
            (out / "requests.jsonl").open("w") as receipts:
        os.chmod(runtime, 0o700)
        env.update(XDG_RUNTIME_DIR=runtime, ULTTY_AUTH_TOKEN="synthetic-local-only")
        env.pop("ULTTY_SOCKET", None)
        endpoint = Path(runtime) / "ultty-session-r.sock"
        admin_path = Path(runtime) / "ultty-session-r.admin"

        def cli(*arguments, check=True):
            completed = subprocess.run([str(binary), *arguments], env=env, capture_output=True, timeout=10)
            receipts.write(json.dumps({"argv": list(arguments), "exit_code": completed.returncode,
                                       "stderr": completed.stderr.decode(errors="replace")[:8192]}) + "\n")
            receipts.flush()
            if check:
                assert completed.returncode == 0, completed.stderr.decode(errors="replace")
            return completed

        def snapshot():
            reply = json.loads(cli("snapshot", "r").stdout)["reply"]
            assert reply["type"] == "snapshot", reply
            return reply["snapshot"]

        def grant(name, caps, ttl="60"):
            path = out / name
            result = json.loads(cli("grant", "r", "--caps", caps, "--ttl", ttl, "--out", str(path)).stdout)
            assert path.stat().st_mode & 0o777 == 0o600
            assert "token" not in result and result["token_file"] == str(path)
            return result["grant_id"], path

        def connect(token, controller=False):
            connection = socket.socket(socket.AF_UNIX)
            connection.settimeout(3)
            connection.connect(str(endpoint))
            hello = {"id": 0, "owner": None, "grant_id": None, "command": {
                "type": "hello", "version": 1, "session": "r", "token": token, "controller": controller,
            }}
            connection.sendall(frame(hello))
            welcome = response(connection)
            assert welcome["reply"]["type"] == "welcome", welcome
            sockets.append(connection)
            return connection, welcome

        def exact(welcome, identity, command):
            return {"id": identity, "owner": welcome["owner"], "grant_id": welcome["reply"]["grant_id"],
                    "command": command}

        try:
            host = subprocess.Popen([str(binary), "host", "r", "-e", "/bin/sleep", "60"],
                                    env=env, stdout=log, stderr=log)
            eventually(lambda: endpoint.exists() and admin_path.exists())
            original = snapshot()
            owner = original["owner"]
            original_pid = original["panes"][0]["pid"]
            reader_id, reader_file = grant("reader.token", "read")
            _, operator_file = grant("operator.token", "read,input,spawn,layout")
            reader = reader_file.read_text().strip()
            operator = operator_file.read_text().strip()

            # The real helper rejects the local owner credential before opening
            # an endpoint, and never emits credential bytes on stdout/stderr.
            admin = admin_path.read_text().strip()
            request = {"id": 0, "owner": None, "grant_id": None, "command": {
                "type": "hello", "version": 1, "session": "r", "token": admin, "controller": False,
            }}
            refused = subprocess.run([str(binary), "attach-stdio"], env=env, input=frame(request),
                                     capture_output=True, timeout=5)
            assert refused.returncode != 0 and not refused.stdout
            assert admin.encode() not in refused.stderr

            # Fake ssh validates argv and then execs the ACTUAL attach-stdio
            # binary. Credentials remain on framed stdin, never argv/environment.
            fake = out / "fake-bin"
            fake.mkdir()
            ssh_log = out / "fake-ssh.jsonl"
            executable = fake / "ssh"
            executable.write_text(
                "#!" + sys.executable + "\n"
                "import json,os,sys\n"
                "assert sys.argv[-1]=='ultty-session attach-stdio'\n"
                "with open(os.environ['FAKE_SSH_LOG'],'a') as f:\n"
                " f.write(json.dumps({'argv':sys.argv[1:],'pid':os.getpid(),"
                "'local_auth_present':'ULTTY_AUTH_TOKEN' in os.environ})+'\\n')\n"
                "os.execv(os.environ['SESSION_BIN'],[os.environ['SESSION_BIN'],'attach-stdio'])\n"
            )
            executable.chmod(0o700)
            env.update(PATH=str(fake) + os.pathsep + env["PATH"], FAKE_SSH_LOG=str(ssh_log),
                       SESSION_BIN=str(binary))
            remote = json.loads(cli("snapshot", "r", "--ssh", "fake-host",
                                    "--token-file", str(reader_file)).stdout)["reply"]["snapshot"]
            assert remote["owner"] == owner and remote["panes"][0]["pid"] == original_pid
            argv_records = [json.loads(line) for line in ssh_log.read_text().splitlines()]
            assert argv_records[-1]["argv"][-3:] == ["--", "fake-host", "ultty-session attach-stdio"]
            assert not argv_records[-1]["local_auth_present"]
            assert all(reader not in argument and operator not in argument
                       for record in argv_records for argument in record["argv"])

            # Read-only authority cannot mutate even over a valid helper.
            observer_input = json.dumps({"type": "input", "pane": original["panes"][0]["id"], "data": [43]}) + "\n"
            refused = subprocess.run([str(binary), "client", "r", "--ssh", "fake-host",
                                      "--token-file", str(reader_file)], env=env,
                                     input=observer_input.encode(), capture_output=True, timeout=10)
            assert refused.returncode != 0
            lines = [json.loads(line) for line in refused.stdout.splitlines()]
            assert lines[-1]["reply"]["type"] == "error" and lines[-1]["reply"]["code"] == "permission_denied"

            # Kill only a helper holding control; host and native/PTY identities survive.
            helper_client = subprocess.Popen([str(binary), "client", "r", "--ssh", "fake-host",
                                               "--token-file", str(operator_file), "--controller"],
                                              env=env, stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=log)
            connected = json.loads(helper_client.stdout.readline())
            assert connected["controller"] and connected["owner"] == owner
            helper_pid = json.loads(ssh_log.read_text().splitlines()[-1])["pid"]
            os.kill(helper_pid, signal.SIGKILL)
            helper_client.stdin.write(b'{"type":"snapshot","after_sequence":null}\n')
            helper_client.stdin.flush()
            helper_client.stdin.close()
            helper_client.wait(timeout=8)
            assert helper_client.returncode != 0 and host.poll() is None
            after_loss = snapshot()
            assert after_loss["owner"] == owner and after_loss["panes"][0]["pid"] == original_pid

            # Reserve a real spawn, observe an external readiness marker but
            # deliberately lose its reply. Reconnect/replay must return the
            # cached receipt without launching a second child.
            time.sleep(0.05)  # permit helper EOF/lease cleanup, not command completion
            connection, welcome = connect(operator, True)
            marker = out / "spawn-ready"
            program = "from pathlib import Path; import sys,time; Path(sys.argv[1]).write_text('ready'); time.sleep(30)"
            operation = exact(welcome, welcome["last_mutation_id"] + 1, {
                "type": "spawn", "program": sys.executable, "args": ["-c", program, str(marker)], "cwd": None,
            })
            connection.sendall(frame(operation))
            eventually(marker.exists)
            before_replay = snapshot()
            assert len(before_replay["panes"]) == len(original["panes"]) + 1
            connection.close()
            reconnect, current = connect(operator)
            reconnect.sendall(frame(operation))
            replayed = response(reconnect)
            assert replayed["reply"]["type"] == "applied", replayed
            child = next(pane for pane in before_replay["panes"] if pane["id"] == replayed["reply"]["pane"])
            after_replay = snapshot()
            assert [(pane["id"], pane["pid"]) for pane in after_replay["panes"]] == [
                (pane["id"], pane["pid"]) for pane in before_replay["panes"]]
            altered = dict(operation, command=dict(operation["command"], args=["-c", "raise RuntimeError('must not run')"]))
            reconnect.sendall(frame(altered))
            assert response(reconnect)["reply"]["code"] == "conflict"
            _, different_file = grant("different.token", "read,spawn")
            different, _ = connect(different_file.read_text().strip())
            different.sendall(frame(operation))
            assert response(different)["reply"]["code"] == "stale_owner"
            assert len(snapshot()["panes"]) == len(before_replay["panes"])

            cli("revoke", "r", str(reader_id))
            assert cli("snapshot", "r", "--token-file", str(reader_file), check=False).returncode != 0
            _, expiring = grant("expires.token", "read", "1")
            time.sleep(1.1)
            assert cli("snapshot", "r", "--token-file", str(expiring), check=False).returncode != 0
            cli("stop", "r")
            assert host.wait(timeout=5) == 0
            elapsed = time.monotonic() - started
            assert elapsed < 60
            (out / "result.json").write_text(json.dumps({"passed": True, "seconds": elapsed, "owner": owner,
                "original_pid": original_pid, "replayed_child_pid": child["pid"], "sha256": hashes,
                "qualification": "local fake ssh execs actual stdio helper; no network SSH/Tailscale test"}, indent=2))
            print(f"session-remote: passed ({out})")
        finally:
            for connection in sockets:
                connection.close()
            for process in (helper_client, host):
                if process is not None and process.poll() is None:
                    process.kill()
                    process.wait(timeout=5)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--session-bin", required=True, type=Path)
    parser.add_argument("--output", required=True, type=Path)
    args = parser.parse_args()
    def expired(_signal, _frame):
        raise TimeoutError("session-remote exceeded fixed 60 second budget")
    signal.signal(signal.SIGALRM, expired)
    signal.alarm(60)
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
