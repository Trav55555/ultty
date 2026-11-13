#!/usr/bin/env python3
"""Bounded REAL SSH qualification against one prestarted, disposable host.

No build, installation, SSH configuration, enrollment or host startup occurs.
Terra's staging step supplies a manifest and makes the fixed remote helper
temporarily PATH-visible under the host's same noninteractive runtime env.
This gate owns its scoped grants and stops exactly the manifest's test host.

Required manifest fields: ssh_host, session, remote_root, remote_session_bin,
remote_admin_file, host_pid, counter_pane, counter_pid, local_session_bin. Optional fields:
remote_runtime, remote_tmpdir, remote_python (default /usr/bin/python3).
Use a fresh output directory. Credentials stay in private files/framed stdin;
receipts contain redacted requests and public owner/process identities only.
"""

import argparse
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import select
import shlex
import shutil
import signal
import struct
import subprocess
import time
import traceback


BUDGET_SECONDS = 180
SSH_OPTIONS = ["-T", "-oBatchMode=yes", "-oConnectTimeout=5",
               "-oServerAliveInterval=5", "-oServerAliveCountMax=1",
               "-oForwardAgent=no", "-oPermitLocalCommand=no"]


def digest(path):
    with open(path, "rb") as source:
        return hashlib.file_digest(source, "sha256").hexdigest()


def private_text(path, value):
    descriptor = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
    with os.fdopen(descriptor, "w") as output:
        output.write(value)


def eventually(action, description, seconds=10):
    deadline = time.monotonic() + seconds
    while time.monotonic() < deadline:
        result = action()
        if result:
            return result
        time.sleep(0.1)
    raise AssertionError(f"timed out: {description}")


def frame(value):
    body = json.dumps(value, separators=(",", ":")).encode()
    assert len(body) <= 1024 * 1024
    return b"UAT1" + struct.pack("<I", len(body)) + body


def read_exact(stream, length, deadline):
    result = bytearray()
    while len(result) < length:
        remaining = deadline - time.monotonic()
        assert remaining > 0, "SSH frame deadline elapsed"
        ready, _, _ = select.select([stream], [], [], remaining)
        assert ready, "SSH frame deadline elapsed"
        part = os.read(stream.fileno(), min(65536, length - len(result)))
        assert part, "SSH helper closed before complete response"
        result.extend(part)
    return result


def read_response(stream):
    deadline = time.monotonic() + 10
    header = read_exact(stream, 8, deadline)
    assert header[:4] == b"UAT1", "wrong SSH attachment magic"
    length = struct.unpack("<I", header[4:])[0]
    assert 0 < length <= 16 * 1024 * 1024, "invalid SSH attachment length"
    return json.loads(read_exact(stream, length, deadline))


def native_text(scene):
    result = []
    for region in scene["regions"]:
        text = region.get("text")
        if not text:
            continue
        if "Rich" in text:
            result.append("".join(span["text"] for span in text["Rich"]))
        else:
            result.extend(value for value in text.values() if isinstance(value, str))
    return result


class RawSsh:
    """Exact framed requests over the SAME fixed helper and real SSH network."""

    def __init__(self, gate, token, controller=False):
        self.gate = gate
        self.process = subprocess.Popen(gate.ssh_argv("ultty-session attach-stdio"),
                                        stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                                        stderr=gate.transport_log, env=gate.local_env, bufsize=0)
        gate.processes.append(self.process)
        hello = {"id": 0, "owner": None, "grant_id": None, "command": {
            "type": "hello", "version": 1, "session": gate.session,
            "token": token, "controller": controller}}
        self.send(hello)
        self.welcome = read_response(self.process.stdout)
        assert self.welcome["reply"]["type"] == "welcome", self.welcome
        assert not self.welcome["reply"]["admin"]
        self.owner = self.welcome["owner"]
        self.grant = self.welcome["reply"]["grant_id"]
        self.next_id = self.welcome["last_mutation_id"] + 1

    def send(self, request):
        self.process.stdin.write(frame(request))
        self.process.stdin.flush()

    def exact(self, request):
        self.send(request)
        response = read_response(self.process.stdout)
        assert response["owner"] == self.owner and response["id"] == request["id"]
        self.gate.receipt("framed_request", request=request, response=response)
        self.next_id = max(self.next_id, response["last_mutation_id"] + 1)
        return response

    def request(self, command, mutation=True):
        request = {"id": self.next_id if mutation else 0, "owner": self.owner,
                   "grant_id": self.grant, "command": command}
        return self.exact(request)

    def close(self):
        if self.process.poll() is None:
            self.process.kill()
        self.process.wait(timeout=5)


class Gate:
    def __init__(self, manifest, out):
        self.manifest = manifest
        self.out = out
        self.binary = Path(manifest["local_session_bin"]).resolve()
        self.host = manifest["ssh_host"]
        self.session = manifest["session"]
        assert self.host and not self.host.startswith("-")
        assert all(character.isascii() and (character.isalnum() or character in "._-@:%[]")
                   for character in self.host)
        assert self.session and self.session[0].isalnum()
        assert all(character.isascii() and (character.isalnum() or character in "._-")
                   for character in self.session)
        self.remote_root = manifest["remote_root"]
        self.remote_binary = manifest["remote_session_bin"]
        for path in (self.remote_root, self.remote_binary):
            assert path.startswith("/") and "\x00" not in path
        self.environment = []
        for field, variable in (("remote_runtime", "XDG_RUNTIME_DIR"), ("remote_tmpdir", "TMPDIR")):
            if manifest.get(field) is not None:
                self.environment.append(variable + "=" + manifest[field])
        self.local_env = dict(os.environ)
        for name in ("ULTTY_AUTH_TOKEN", "ULTTY_SOCKET", "ULTTY_PID", "ULTTY_OWNER_EPOCH"):
            self.local_env.pop(name, None)
        self.processes = []
        self.grants = []
        self.secrets = []
        self.local_tokens = []
        self.marker_owned = False
        self.owned_children = [manifest["counter_pid"]]
        self.receipts = (out / "receipts.jsonl").open("w")
        self.transport_log = (out / "ssh-stderr.log").open("wb")

    def receipt(self, event, **values):
        record = json.dumps({"event": event, **values})
        for token in self.secrets:
            record = record.replace(token, "<redacted>")
        self.receipts.write(record + "\n")
        self.receipts.flush()

    def ssh_argv(self, command):
        return ["ssh", *SSH_OPTIONS, "--", self.host, command]

    def remote(self, arguments, check=True, secret=False):
        result = subprocess.run(self.ssh_argv(shlex.join(arguments)), capture_output=True,
                                timeout=15, env=self.local_env)
        self.receipt("remote", argv=arguments, exit_code=result.returncode,
                     stdout="<credential>" if secret else result.stdout.decode(errors="replace")[:16384],
                     stderr=result.stderr.decode(errors="replace")[:8192])
        if check:
            assert result.returncode == 0, "remote command refused; see redacted receipt"
        return result

    def admin(self, *arguments, check=True):
        return self.remote(["env", *self.environment, self.remote_binary,
                            arguments[0], self.session, *arguments[1:]], check=check)

    def grant(self, label, capabilities, ttl=120):
        remote_file = self.remote_root + "/ssh-gate-" + label + ".token"
        response = json.loads(self.admin("grant", "--caps", capabilities, "--ttl", str(ttl),
                                         "--out", remote_file).stdout)
        grant_id = response["grant_id"]
        self.grants.append((grant_id, remote_file))
        token = self.remote(["cat", "--", remote_file], secret=True).stdout.decode().strip()
        assert token and not token.startswith("0.") and len(token) <= 128
        self.secrets.append(token)
        local_file = self.out / (label + ".token")
        self.local_tokens.append(local_file)
        private_text(local_file, token + "\n")
        return grant_id, token, local_file

    def cli(self, *arguments, check=True, stdin=None):
        result = subprocess.run([str(self.binary), *arguments], input=stdin,
                                capture_output=True, timeout=20, env=self.local_env)
        self.receipt("rust_connect_ssh", argv=list(arguments), exit_code=result.returncode,
                     stderr=result.stderr.decode(errors="replace")[:8192])
        if check:
            assert result.returncode == 0, "local Rust SSH client refused; see redacted receipt"
        return result

    def snapshot(self, credential):
        response = json.loads(self.cli("snapshot", self.session, "--ssh", self.host,
                                       "--token-file", str(credential)).stdout)
        assert response["reply"]["type"] == "snapshot", response
        assert response["owner"] == response["reply"]["snapshot"]["owner"]
        return response["reply"]["snapshot"]

    def gui(self, binary, credential):
        """Reuse the canonical private Xvfb and native-scene PNG oracle."""
        source = Path(__file__).with_name("session-viewer.py")
        spec = importlib.util.spec_from_file_location("real_ssh_viewer_oracle", source)
        oracle = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(oracle)
        env = dict(self.local_env)
        env.pop("WAYLAND_DISPLAY", None)
        runtime = self.out / "gui-runtime"
        runtime.mkdir(mode=0o700)
        config = self.out / "gui-config" / "ultty"
        config.mkdir(parents=True)
        (config / "config.toml").write_text("[behavior]\ncursor_blink = false\n")
        env.update(XDG_RUNTIME_DIR=str(runtime), XDG_CONFIG_HOME=str(config.parent),
                   RUST_LOG="info,wgpu_core=warn,wgpu_hal=warn")
        drivers = sorted(Path("/usr/share/vulkan/icd.d").glob("lvp_icd*.json"))
        if "VK_ICD_FILENAMES" not in env and drivers:
            env["VK_ICD_FILENAMES"] = str(drivers[0])
        with (self.out / "display").open("w+b") as display, (self.out / "xvfb.log").open("wb") as log:
            xvfb = subprocess.Popen(["Xvfb", "-displayfd", str(display.fileno()), "-screen", "0",
                                     "1280x800x24", "-nolisten", "tcp", "-noreset"],
                                    pass_fds=(display.fileno(),), stdout=log, stderr=log)
            self.processes.append(xvfb)
            def ready():
                assert xvfb.poll() is None, "private Xvfb exited before handoff"
                value = (self.out / "display").read_text()
                return value.strip() if value.endswith("\n") and value.strip().isdigit() else None
            env["DISPLAY"] = ":" + eventually(ready, "private Xvfb display")
            image = self.out / "remote-counter.png"
            with (self.out / "gui.log").open("wb") as gui_log:
                viewer = subprocess.Popen([str(binary), "--attach", self.session, "--ssh", self.host,
                                           "--observe", "--token-file", str(credential),
                                           "--exit-after-ms", "5000", "--screenshot", str(image)],
                                          env=env, stdout=gui_log, stderr=gui_log)
                self.processes.append(viewer)
                assert viewer.wait(timeout=25) == 0, "real SSH GPU viewer failed"
            screenshot = oracle.screenshot(image)
            xvfb.terminate()
            xvfb.wait(timeout=5)
        return {"binary": str(binary), "binary_sha256": digest(binary),
                "oracle_sha256": digest(source), **screenshot}

    def cleanup(self):
        for path in self.local_tokens:
            path.unlink(missing_ok=True)
        for process in self.processes:
            if process.poll() is None:
                process.kill()
            process.wait(timeout=5)
        for grant, token_file in reversed(self.grants):
            self.admin("revoke", str(grant), check=False)
            self.remote(["rm", "-f", "--", token_file], check=False)
        # Only the manifest's disposable session is stopped; staging/helper
        # symlink cleanup belongs to the separate staging owner.
        current = self.admin("snapshot", check=False)
        if current.returncode == 0:
            owner = json.loads(current.stdout)["owner"]
            assert owner["pid"] == self.manifest["host_pid"], "cleanup refused replacement owner"
            self.admin("stop")
        def stopped():
            return self.remote(["kill", "-0", str(self.manifest["host_pid"])], check=False).returncode != 0
        eventually(stopped, "owned remote host termination", 10)
        for pid in self.owned_children:
            eventually(lambda: self.remote(["kill", "-0", str(pid)], check=False).returncode != 0,
                       "owned remote child termination", 5)
        if self.marker_owned:
            self.remote(["rm", "-f", "--", self.remote_root + "/ssh-gate-spawn.ready"], check=False)
        self.receipts.close()
        self.transport_log.close()


def run(args):
    manifest = json.loads(args.manifest.read_text())
    out = args.output.resolve()
    out.mkdir(parents=True, exist_ok=False, mode=0o700)
    os.chmod(out, 0o700)
    gate = Gate(manifest, out)
    started = time.monotonic()
    result = {"passed": False, "budget_seconds": BUDGET_SECONDS,
              "qualification": "actual SSH network; Rust connect_ssh plus exact framed helper replay"}
    try:
        (out / "session-ssh.py").write_bytes(Path(__file__).read_bytes())
        (out / "inputs.json").write_text(json.dumps({"manifest": manifest, "sha256": {
            str(gate.binary): digest(gate.binary), str(Path(__file__).resolve()): digest(__file__)},
            "budget_seconds": BUDGET_SECONDS, "cleanup_budget_seconds": 45}, indent=2) + "\n")
        executable = Path(shutil.which("ssh") or "").resolve()
        version = subprocess.run([str(executable), "-V"], capture_output=True, timeout=5)
        assert version.returncode == 0 and b"OpenSSH" in version.stderr + version.stdout
        gate.receipt("real_ssh_executable", path=str(executable), sha256=digest(executable),
                     version=(version.stderr + version.stdout).decode(errors="replace").strip())
        helper = gate.remote(["sh", "-c", "command -v ultty-session; printf 'runtime=%s\\ntmp=%s\\n' \"$XDG_RUNTIME_DIR\" \"$TMPDIR\""])
        assert helper.stdout.splitlines()[0].startswith(b"/")
        gate.remote(["test", "!", "-e", gate.remote_root + "/ssh-gate-spawn.ready"])
        remote_hash = gate.remote(["sha256sum", "--", gate.remote_binary]).stdout.decode().split()[0]
        result["remote_binary_sha256"] = remote_hash
        reader_id, reader, reader_file = gate.grant("reader", "read")
        _, operator, operator_file = gate.grant("operator", "read,input,spawn,layout")
        first = eventually(lambda: gate.snapshot(reader_file), "real Rust SSH snapshot", 20)
        owner = first["owner"]
        assert owner["pid"] == manifest["host_pid"]
        counter = next(pane for pane in first["panes"] if pane["id"] == manifest["counter_pane"])
        assert counter["pid"] == manifest["counter_pid"] and counter["native"]
        native = counter["native"][0]
        assert native["full_scene"] and "0" in native_text(native)
        result.update(owner=owner, counter_pid=counter["pid"], native_session=native["session_id"])

        readonly = RawSsh(gate, reader)
        denied = readonly.request({"type": "input", "pane": counter["id"], "data": [43]})
        assert denied["reply"]["code"] == "permission_denied"
        readonly.close()
        controller = RawSsh(gate, operator, True)
        assert controller.owner == owner
        busy = subprocess.Popen(gate.ssh_argv("ultty-session attach-stdio"), stdin=subprocess.PIPE,
                                stdout=subprocess.PIPE, stderr=gate.transport_log, env=gate.local_env, bufsize=0)
        gate.processes.append(busy)
        busy.stdin.write(frame({"id": 0, "owner": None, "grant_id": None, "command": {
            "type": "hello", "version": 1, "session": gate.session, "token": operator, "controller": True}}))
        busy.stdin.flush()
        assert read_response(busy.stdout)["reply"]["code"] == "busy"
        busy.wait(timeout=5)
        # Pointer input is fenced; keyboard input follows exact current focus.
        payload = struct.pack("<IBffB", 0, 2, 0.0, 0.0, 0)
        mouse = b"UT" + struct.pack("<HI", 0x20, len(payload)) + payload
        stale = controller.request({"type": "native_input", "session": native["session_id"],
                                    "generation": native["generation"] + 1000000, "payload": list(mouse)})
        assert stale["reply"]["code"] == "conflict"
        payload = struct.pack("<IBBIBffB", 0, 4, 0, ord("+"), 0, 0.0, 0.0, 0)
        keyboard = b"UT" + struct.pack("<HI", 0x20, len(payload)) + payload
        applied = controller.request({"type": "native_input", "session": native["session_id"],
                                      "generation": native["generation"], "payload": list(keyboard)})
        assert applied["reply"]["type"] == "ok"
        def incremented():
            current = gate.snapshot(reader_file)
            pane = next(pane for pane in current["panes"] if pane["id"] == counter["id"])
            scene = next(scene for scene in pane["native"] if scene["session_id"] == native["session_id"])
            assert pane["pid"] == counter["pid"] and current["owner"] == owner
            if "1" in native_text(scene):
                assert scene["generation"] > native["generation"]
                return current
            return None
        eventually(incremented, "remote native counter update")
        controller.close()

        # Production Rust connect_ssh owns an actual ssh child. Kill only that
        # transport, then require the owner and native process identities survive.
        time.sleep(0.2)
        rust = subprocess.Popen([str(gate.binary), "client", gate.session, "--ssh", gate.host,
                                 "--token-file", str(operator_file), "--controller"],
                                stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                                stderr=gate.transport_log, env=gate.local_env, bufsize=0)
        gate.processes.append(rust)
        ready, _, _ = select.select([rust.stdout], [], [], 15)
        assert ready, "Rust SSH connection did not complete"
        connected = json.loads(rust.stdout.readline())
        assert connected["controller"] and connected["owner"] == owner
        children = Path(f"/proc/{rust.pid}/task/{rust.pid}/children").read_text().split()
        assert len(children) == 1, "expected the Rust client's sole SSH child"
        os.kill(int(children[0]), signal.SIGKILL)
        rust.stdin.write(b'{"type":"snapshot","after_sequence":null}\n')
        rust.stdin.flush()
        rust.stdin.close()
        assert rust.wait(timeout=10) != 0
        after_loss = gate.snapshot(reader_file)
        retained = next(pane for pane in after_loss["panes"] if pane["id"] == counter["id"])
        assert after_loss["owner"] == owner and retained["pid"] == counter["pid"]
        assert any(scene["session_id"] == native["session_id"] and "1" in native_text(scene)
                   for scene in retained["native"])
        if args.gui_bin:
            result["gui"] = gate.gui(args.gui_bin.resolve(), reader_file)
            after_gui = gate.snapshot(reader_file)
            retained = next(pane for pane in after_gui["panes"] if pane["id"] == counter["id"])
            assert after_gui["owner"] == owner and retained["pid"] == counter["pid"]
            assert any(scene["session_id"] == native["session_id"] and "1" in native_text(scene)
                       for scene in retained["native"])

        # Deliberately lose an accepted spawn receipt over real SSH, then replay
        # exact owner/grant/operation identity without launching another process.
        time.sleep(0.2)
        lost = RawSsh(gate, operator, True)
        marker = gate.remote_root + "/ssh-gate-spawn.ready"
        program = "import os,pathlib,sys,time;pathlib.Path(sys.argv[1]).write_text(str(os.getpid()));time.sleep(60)"
        operation = {"id": lost.next_id, "owner": owner, "grant_id": lost.grant, "command": {
            "type": "spawn", "program": manifest.get("remote_python", "/usr/bin/python3"),
            "args": ["-c", program, marker], "cwd": None}}
        lost.send(operation)
        gate.marker_owned = True
        marker_pid = int(eventually(lambda: gate.remote(["cat", "--", marker], check=False).stdout.strip(),
                                    "accepted remote spawn marker"))
        gate.owned_children.append(marker_pid)
        before_replay = gate.snapshot(reader_file)
        assert len(before_replay["panes"]) == len(first["panes"]) + 1
        hidden = next(pane for pane in before_replay["panes"] if pane["id"] == counter["id"])
        assert before_replay["active_tab"] != hidden["tab"]
        assert hidden["pid"] == counter["pid"]
        assert any(scene["session_id"] == native["session_id"] and "1" in native_text(scene)
                   for scene in hidden["native"]), "hidden counter lost its committed native session/state"
        result["hidden_native_retained"] = True
        lost.close()
        replay = RawSsh(gate, operator)
        receipt = replay.exact(operation)
        assert receipt["reply"]["type"] == "applied"
        spawned = next(pane for pane in before_replay["panes"] if pane["id"] == receipt["reply"]["pane"])
        assert spawned["pid"] == marker_pid
        after_replay = gate.snapshot(reader_file)
        identities = lambda snapshot: sorted((pane["id"], pane["pid"]) for pane in snapshot["panes"])
        assert identities(before_replay) == identities(after_replay)
        altered = dict(operation, command=dict(operation["command"], args=["-c", "raise RuntimeError('must not execute')"]))
        assert replay.exact(altered)["reply"]["code"] == "conflict"
        stale_owner = dict(operation, owner=dict(owner, epoch="0" * 64 if owner["epoch"] != "0" * 64 else "1" * 64))
        assert replay.exact(stale_owner)["reply"]["code"] == "stale_owner"
        replay.close()
        cross_grant = RawSsh(gate, reader)
        assert cross_grant.exact(operation)["reply"]["code"] == "stale_owner"
        cross_grant.close()
        result["replayed_child_pid"] = marker_pid

        # An actual remote admin token is tested ONLY on the remote machine;
        # it is never downloaded or forwarded across the SSH attachment route.
        admin_test = """import json,pathlib,struct,subprocess,sys
token=pathlib.Path(sys.argv[2]).read_text().strip()
request={'id':0,'owner':None,'grant_id':None,'command':{'type':'hello','version':1,'session':sys.argv[3],'token':token,'controller':False}}
body=json.dumps(request).encode()
result=subprocess.run([sys.argv[1],'attach-stdio'],input=b'UAT1'+struct.pack('<I',len(body))+body,capture_output=True,timeout=5)
assert result.returncode and not result.stdout and token.encode() not in result.stderr
print('admin helper refused without credential disclosure')
"""
        assert manifest.get("remote_admin_file"), "manifest needs remote admin path for refusal gate"
        gate.remote(["env", *gate.environment, manifest.get("remote_python", "/usr/bin/python3"),
                     "-c", admin_test, gate.remote_binary, manifest["remote_admin_file"], gate.session])
        dummy_admin = out / "synthetic-admin.token"
        gate.local_tokens.append(dummy_admin)
        private_text(dummy_admin, "0." + "0" * 64 + "\n")
        assert gate.cli("snapshot", gate.session, "--ssh", gate.host,
                        "--token-file", str(dummy_admin), check=False).returncode != 0
        gate.admin("revoke", str(reader_id))
        assert gate.cli("snapshot", gate.session, "--ssh", gate.host,
                        "--token-file", str(reader_file), check=False).returncode != 0
        _, _, expired = gate.grant("expired", "read", ttl=1)
        time.sleep(1.2)
        assert gate.cli("snapshot", gate.session, "--ssh", gate.host,
                        "--token-file", str(expired), check=False).returncode != 0
        result["passed"] = True
    except Exception:
        text = traceback.format_exc()
        for token in gate.secrets:
            text = text.replace(token, "<redacted>")
        (out / "failure.log").write_text(text)
        raise
    finally:
        signal.alarm(45)  # cleanup is bounded separately, even after a gate timeout
        try:
            gate.cleanup()
            result["cleanup"] = "temporary grants revoked; scoped token files removed; owned test host confirmed stopped"
        except Exception:
            result["passed"] = False
            result["cleanup"] = "failed; staging owner must inspect retained receipts"
            with (out / "failure.log").open("a") as output:
                output.write("\nCleanup failure:\n" + traceback.format_exc())
            raise
        finally:
            result["seconds"] = time.monotonic() - started
            (out / "result.json").write_text(json.dumps(result, indent=2) + "\n")
    assert result["seconds"] < BUDGET_SECONDS + 45
    print(f"session-ssh: passed ({out})")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--manifest", required=True, type=Path)
    parser.add_argument("--output", required=True, type=Path)
    parser.add_argument("--gui-bin", type=Path, help="Optionally qualify the real SSH GPU viewer under private Xvfb")
    args = parser.parse_args()
    def expired(_signal, _frame):
        raise TimeoutError("session-ssh exceeded bounded deadline")
    signal.signal(signal.SIGALRM, expired)
    signal.alarm(BUDGET_SECONDS)
    try:
        run(args)
    finally:
        signal.alarm(0)


if __name__ == "__main__":
    main()
