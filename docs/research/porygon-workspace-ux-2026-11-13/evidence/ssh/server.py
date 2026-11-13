"""One-shot loopback-only SSH fixture. Runs only the pinned candidate in a PTY."""
import base64
import fcntl
import json
import os
import pty
import select
import signal
import socket
import struct
import subprocess
import sys
import termios
import threading

import paramiko

host_key, authorized_key, binary, ready_path, resize_path = sys.argv[1:]
key = paramiko.RSAKey.from_private_key_file(host_key)
allowed = paramiko.RSAKey(data=base64.b64decode(open(authorized_key, encoding="utf-8").read().split()[1]))

class Server(paramiko.ServerInterface):
    def __init__(self):
        self.event = threading.Event()
        self.width, self.height = 80, 24
        self.master = None

    def check_auth_publickey(self, username, public_key):
        return paramiko.AUTH_SUCCESSFUL if username == "fixture" and public_key == allowed else paramiko.AUTH_FAILED

    def get_allowed_auths(self, username):
        return "publickey"

    def check_channel_request(self, kind, chanid):
        return paramiko.OPEN_SUCCEEDED if kind == "session" else paramiko.OPEN_FAILED_ADMINISTRATIVELY_PROHIBITED

    def check_channel_pty_request(self, channel, term, width, height, pixelwidth, pixelheight, modes):
        self.width, self.height = width, height
        return True

    def check_channel_shell_request(self, channel):
        self.event.set()
        return True

    def check_channel_window_change_request(self, channel, width, height, pixelwidth, pixelheight):
        self.width, self.height = width, height
        with open(resize_path, "a", encoding="utf-8") as output:
            output.write(json.dumps({"width": width, "height": height}) + "\n")
        if self.master is not None:
            fcntl.ioctl(self.master, termios.TIOCSWINSZ, struct.pack("HHHH", height, width, 0, 0))
        return True

listener = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
listener.bind(("127.0.0.1", 0))
listener.listen(1)
with open(ready_path, "w", encoding="utf-8") as output:
    output.write(str(listener.getsockname()[1]))
client, _ = listener.accept()
listener.close()
transport = paramiko.Transport(client)
transport.add_server_key(key)
server = Server()
transport.start_server(server=server)
channel = transport.accept(15)
if channel is None or not server.event.wait(15):
    raise SystemExit("SSH session or shell request missing")
master, slave = pty.openpty()
server.master = master
fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", server.height, server.width, 0, 0))
proc = subprocess.Popen([binary], stdin=slave, stdout=slave, stderr=slave, env=os.environ.copy(), start_new_session=True, preexec_fn=lambda: fcntl.ioctl(0, termios.TIOCSCTTY, 0))
os.close(slave)
try:
    while proc.poll() is None and transport.is_active() and not channel.closed:
        readable, _, _ = select.select([master, channel], [], [], .1)
        if channel in readable:
            data = channel.recv(65536)
            if not data:
                break
            os.write(master, data)
        if master in readable:
            try:
                data = os.read(master, 65536)
            except OSError:
                break
            if not data:
                break
            channel.sendall(data)
finally:
    if proc.poll() is None:
        os.killpg(proc.pid, signal.SIGTERM)
        proc.wait(timeout=5)
    channel.close()
    transport.close()
    os.close(master)
