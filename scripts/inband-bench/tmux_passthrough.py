"""Does an APC 'U' frame cross tmux 3.x? Both directions, passthrough on/off.

The outer terminal is a PTY this script holds; tmux's client runs on it.
"""
import base64, os, pty, select, subprocess, sys, time, tty

SOCK = "inbandbench"
TMUX = ["tmux", "-L", SOCK, "-f", "/dev/null"]
DIR = os.path.dirname(os.path.abspath(__file__))

def drain(fd, secs):
    out, end = bytearray(), time.time() + secs
    while time.time() < end:
        r, _, _ = select.select([fd], [], [], 0.05)
        if r:
            try: out += os.read(fd, 1 << 20)
            except OSError: break
    return bytes(out)

def attach():
    pid, fd = pty.fork()
    if pid == 0:
        os.environ["TERM"] = "xterm-256color"
        os.execvp("tmux", TMUX + ["attach"])
    os.set_blocking(fd, False)
    drain(fd, 1.0)
    return pid, fd

def apc(payload): return b"\x1b_U" + payload + b"\x1b\\"
def dcs_wrap(seq): return b"\x1bPtmux;" + seq.replace(b"\x1b", b"\x1b\x1b") + b"\x1b\\"

def run(passthrough):
    subprocess.run(TMUX + ["kill-server"], capture_output=True)
    subprocess.run(TMUX + ["new-session", "-d", "-x", "80", "-y", "24", "sh"], check=True)
    subprocess.run(TMUX + ["set", "-g", "allow-passthrough", "on" if passthrough else "off"], check=True)
    pid, fd = attach()
    results = {}
    for size in (100, 1_400_000):
        payload = base64.urlsafe_b64encode(os.urandom(size * 3 // 4)).rstrip(b"=")[:size]
        for label, wire in (("bare APC", apc(payload)), ("DCS-wrapped", dcs_wrap(apc(payload)))):
            path = os.path.join(DIR, "tmux_payload.bin")
            open(path, "wb").write(wire)
            subprocess.run(TMUX + ["send-keys", f"cat {path}; clear", "Enter"], check=True)
            seen = drain(fd, 3.0 if size > 1000 else 1.0)
            results[f"app->terminal {label} {size} B"] = apc(payload) in seen
    # Terminal -> app: write an APC reply into the outer terminal's input.
    out = os.path.join(DIR, "tmux_pane_input.bin")
    subprocess.run(TMUX + ["send-keys", f"stty raw -echo; head -c 200 > {out}; stty sane", "Enter"], check=True)
    time.sleep(0.5)
    reply = apc(b"REPLYPAYLOAD")
    os.write(fd, reply + b"x" * 200)
    time.sleep(1.0)
    got = open(out, "rb").read() if os.path.exists(out) else b""
    results["terminal->app APC reaches pane"] = reply in got
    results["terminal->app pane received (first 40 B)"] = got[:40]
    os.kill(pid, 9)
    subprocess.run(TMUX + ["kill-server"], capture_output=True)
    for f in ("tmux_payload.bin", "tmux_pane_input.bin"):
        path = os.path.join(DIR, f)
        if os.path.exists(path):
            os.unlink(path)
    return results

for passthrough in (False, True):
    print(f"allow-passthrough {'on' if passthrough else 'off'}:")
    for k, v in run(passthrough).items():
        print(f"  {k}: {v}")
