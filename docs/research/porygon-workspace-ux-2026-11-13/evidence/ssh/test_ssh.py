"""OpenSSH client and fixed Porygon binary through an authenticated loopback SSH PTY."""
import fcntl
import os
import pty
import signal
import struct
import subprocess
import termios
import time
from pathlib import Path

import pytest
from compose_support import PtyApp, TOKEN
from test_workspace_pty import binary, daemon

pytest_plugins = ["conftest"]

SSH_DIR = Path(__file__).resolve().parent

class SshApp(PtyApp):
    def __init__(self, binary_path, width, app, session, tmp_path):
        super().__init__(binary_path, width, app, session)
        self.keys = tmp_path / "keys"
        self.keys.mkdir(mode=0o700)
        self.remote = None

    def __enter__(self):
        self.thread.start()
        deadline = time.monotonic() + 10
        while not self.server.started and self.thread.is_alive() and time.monotonic() < deadline:
            time.sleep(.01)
        assert self.server.started
        for name in ("host", "client"):
            subprocess.run(["/usr/bin/ssh-keygen", "-q", "-t", "rsa", "-b", "3072", "-m", "PEM", "-N", "", "-f", str(self.keys / name)], check=True, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        ready = self.keys / "ready"
        resize = self.keys / "resize.jsonl"
        env = os.environ.copy()
        env.update(TERM="xterm-256color", PORYGON_TOKEN=TOKEN, PORYGON_URL=f"http://127.0.0.1:{self.listener.getsockname()[1]}", PORYGON_WEATHER="0", PORYGON_CONTRACT_FIXTURE="1", XDG_STATE_HOME=str(self.keys / "state"))
        for key in ("ULTTY_SOCKET", "ULTTY_TTY", "ULTTY_FORCE_NATIVE", "ULTTY_IN_BAND", "TMUX", "STY"):
            env.pop(key, None)
        self.remote = subprocess.Popen([str(SSH_DIR / ".venv/bin/python"), str(SSH_DIR / "server.py"), str(self.keys / "host"), str(self.keys / "client.pub"), str(self.binary), str(ready), str(resize)], env=env, stdout=subprocess.DEVNULL, stderr=(self.keys / "server.err").open("w"))
        deadline = time.monotonic() + 10
        while not ready.exists() and self.remote.poll() is None and time.monotonic() < deadline:
            time.sleep(.01)
        assert ready.exists(), (self.keys / "server.err").read_text()
        port = ready.read_text()
        pub = (self.keys / "host.pub").read_text().split()
        known = self.keys / "known_hosts"
        known.write_text(f"[127.0.0.1]:{port} {pub[0]} {pub[1]}\n")
        self.master, self.slave = pty.openpty()
        fcntl.ioctl(self.slave, termios.TIOCSWINSZ, struct.pack("HHHH", self.height, self.width, 0, 0))
        self.proc = subprocess.Popen(["/usr/bin/ssh", "-F", "/dev/null", "-tt", "-o", "BatchMode=yes", "-o", "IdentitiesOnly=yes", "-o", "StrictHostKeyChecking=yes", "-o", f"UserKnownHostsFile={known}", "-o", "GlobalKnownHostsFile=/dev/null", "-o", "PreferredAuthentications=publickey", "-i", str(self.keys / "client"), "-p", port, "fixture@127.0.0.1"], stdin=self.slave, stdout=self.slave, stderr=self.slave, env=env, start_new_session=True, preexec_fn=lambda: fcntl.ioctl(0, termios.TIOCSCTTY, 0))
        os.close(self.slave)
        self.slave = None
        return self

    def __exit__(self, *args):
        try:
            super().__exit__(*args)
        finally:
            if self.remote and self.remote.poll() is None:
                self.remote.terminate()
                self.remote.wait(timeout=5)

@pytest.mark.timeout(60)
@pytest.mark.asyncio
async def test_real_ssh_resize_and_paste(mock_context, mock_database, monkeypatch, tmp_path):
    app, session = daemon(mock_context, monkeypatch)
    width = int(os.environ.get("PORYGON_ACCEPTANCE_WIDTH", "80"))
    with SshApp(binary(), width, app, session, tmp_path) as term:
        term.wait_screen("Message Porygon")
        term.send(b"\x1b[200~ssh-paste-first\nssh-paste-second\x1b[201~")
        term.wait_screen("ssh-paste-first")
        assert session.messages == [], "paste submitted a turn"
        next_width = 160 if width == 80 else 80
        term.resize(next_width, 40 if next_width == 160 else 24)
        term.collect(.5)
        term.wait_screen("ssh-paste-first")
        term.send(b"\r")
        term.wait_calls(1)
        term.collect(.5)
        assert session.messages == ["ssh-paste-first\nssh-paste-second"], session.messages
        resize_file = term.keys / "resize.jsonl"
        deadline = time.monotonic() + 3
        while not resize_file.exists() and time.monotonic() < deadline:
            time.sleep(.05)
        assert resize_file.exists() and f'"width": {next_width}' in resize_file.read_text()
        term.save_stage("ssh-after-resize-and-submit")
