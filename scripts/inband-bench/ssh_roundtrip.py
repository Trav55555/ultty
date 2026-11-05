"""Check that in-band APC frames cross a real ssh hop, and time them.

The remote side plays the app: its tty is raw, as a running app's is. The
local side plays the terminal: it runs `ssh -tt` on a PTY it holds, the way
Ultty runs a shell in a tab.

    python3 ssh_roundtrip.py local <corpus.bin> ssh [ssh-args...] <host>
    (it copies itself and the corpus to the host and runs `remote` there)

Every integrity frame carries a SHA-256 of its data, so each side checks the
other's frames independently. Frame kinds (first payload byte):
  I  app -> terminal integrity frame: sha256 + data
  Q  app asks for terminal -> app frames: JSON list of sizes
  R  terminal -> app integrity frame: sha256 + data
  P  ping: reply size u32 LE + data; answered with a pong R frame
  S  app's JSON summary; the run ends
A bare 0x01 byte is a raw ping, answered with a bare 0x02 (baseline).
"""
import base64, hashlib, json, os, pty, select, struct, subprocess, sys, time, tty

APC, ST = b"\x1b_U", b"\x1b\\"
CHUNK = 64 * 1024


def wrap(raw):
    return APC + base64.urlsafe_b64encode(raw).rstrip(b"=") + ST


def unb64(p):
    return base64.urlsafe_b64decode(p + b"=" * (-len(p) % 4))


class Extractor:
    """Pull APC U payloads out of a stream split at arbitrary points."""

    def __init__(self):
        self.buf = bytearray()

    def feed(self, data):
        """Return (payloads, bare bytes outside any APC)."""
        self.buf += data
        payloads, bare = [], bytearray()
        while True:
            start = self.buf.find(APC)
            if start < 0:
                keep = 2 if self.buf.endswith(b"\x1b_") else 1 if self.buf.endswith(b"\x1b") else 0
                bare += self.buf[: len(self.buf) - keep]
                del self.buf[: len(self.buf) - keep]
                return payloads, bytes(bare)
            bare += self.buf[:start]
            end = self.buf.find(ST, start + 3)
            if end < 0:
                del self.buf[:start]
                return payloads, bytes(bare)
            payloads.append(bytes(self.buf[start + 3 : end]))
            del self.buf[: end + 2]


def integrity(kind, data):
    return kind + hashlib.sha256(data).digest() + data


def check(raw):
    return hashlib.sha256(raw[33:]).digest() == raw[1:33]


def pct(xs, p):
    xs = sorted(xs)
    return xs[round((len(xs) - 1) * p)]


# ------------------------------------------------------------------ remote


def remote(corpus_path):
    fd_in, out = sys.stdin.fileno(), sys.stdout.buffer
    tty.setraw(fd_in)
    ex = Extractor()

    def send(raw):
        out.write(wrap(raw))
        out.flush()

    def recv_payloads(n):
        got = []
        while len(got) < n:
            p, _ = ex.feed(os.read(fd_in, 1 << 20))
            got += p
        return got

    records = []
    data = open(corpus_path, "rb").read()
    i = 0
    while i < len(data):
        d, _, n = struct.unpack_from("<BBI", data, i)
        records.append((d, data[i + 6 : i + 6 + n]))
        i += 6 + n
    updates = [b for d, b in records if d == 0]
    replies = [b for d, b in records if d == 1]
    big = os.urandom(1 << 20)
    single = os.urandom(1_400_000)

    # App -> terminal: every real update, 1 MiB in 64 KiB chunks, 1.4 MB whole.
    frames = [integrity(b"I", u) for u in updates]
    frames += [integrity(b"I", big[o : o + CHUNK]) for o in range(0, len(big), CHUNK)]
    frames.append(integrity(b"I", single))
    t0 = time.monotonic()
    for f in frames:
        send(f)
    send(b"Q" + json.dumps([len(r) for r in replies] + [CHUNK] * 16 + [1_400_000]).encode())
    got = recv_payloads(len(replies) + 17)
    bulk_s = time.monotonic() - t0
    reply_ok = sum(check(unb64(p)) for p in got)

    # Ping-pong: recorded median and largest exchanges.
    updates.sort(key=len)
    replies.sort(key=len)
    cases = {
        "median": (updates[len(updates) // 2], len(replies[len(replies) // 2])),
        "largest": (updates[-1], len(replies[-1])),
    }
    lat = {}
    for name, (req, rep_len) in cases.items():
        samples = []
        for k in range(550):
            t = time.monotonic_ns()
            send(b"P" + struct.pack("<I", rep_len) + req)
            recv_payloads(1)
            if k >= 50:
                samples.append(time.monotonic_ns() - t)
        lat[name] = {"req": len(req), "rep": rep_len, "samples": samples}
    raw = []
    for k in range(550):
        t = time.monotonic_ns()
        out.write(b"\x01")
        out.flush()
        while True:
            _, bare = ex.feed(os.read(fd_in, 64))
            if b"\x02" in bare:
                break
        if k >= 50:
            raw.append(time.monotonic_ns() - t)
    lat["raw 1-byte baseline"] = {"req": 1, "rep": 1, "samples": raw}

    summary = {
        "app_to_terminal_frames": len(frames),
        "terminal_to_app_frames": len(got),
        "terminal_to_app_ok": reply_ok,
        "bulk_phase_s": bulk_s,
        "latency": {
            k: {"req": v["req"], "rep": v["rep"], "p50": pct(v["samples"], 0.5), "p99": pct(v["samples"], 0.99), "max": max(v["samples"])}
            for k, v in lat.items()
        },
    }
    send(b"S" + json.dumps(summary).encode())


# ------------------------------------------------------------------- local


def local(corpus_path, ssh):
    here = os.path.abspath(__file__)
    subprocess.run(
        ["scp", "-q", *ssh[1:-1], here, corpus_path, f"{ssh[-1]}:/tmp/"], check=True
    )
    remote_cmd = "python3 /tmp/ssh_roundtrip.py remote /tmp/" + os.path.basename(corpus_path)
    pid, fd = pty.fork()
    if pid == 0:
        os.execvp(ssh[0], [*ssh[:-1], "-tt", "-e", "none", ssh[-1], remote_cmd])
    ex = Extractor()
    ok = bad = 0
    summary = None
    deadline = time.time() + 300
    while summary is None and time.time() < deadline:
        r, _, _ = select.select([fd], [], [], 1.0)
        if not r:
            continue
        try:
            data = os.read(fd, 1 << 20)
        except OSError:
            break
        payloads, bare = ex.feed(data)
        if b"\x01" in bare:
            os.write(fd, b"\x02" * bare.count(b"\x01"))
        for p in payloads:
            raw = unb64(p)
            kind = raw[:1]
            if kind == b"I":
                ok, bad = (ok + 1, bad) if check(raw) else (ok, bad + 1)
            elif kind == b"Q":
                for size in json.loads(raw[1:]):
                    os.write(fd, wrap(integrity(b"R", os.urandom(size))))
            elif kind == b"P":
                (n,) = struct.unpack_from("<I", raw, 1)
                os.write(fd, wrap(b"R" + os.urandom(n)))
            elif kind == b"S":
                summary = json.loads(raw[1:])
    os.kill(pid, 9)
    if summary is None:
        sys.exit("no summary from remote")
    print(f"app -> terminal frames verified: {ok} ok, {bad} corrupt")
    print(
        f"terminal -> app frames verified: {summary['terminal_to_app_ok']} ok of "
        f"{summary['terminal_to_app_frames']}"
    )
    print(f"bulk phase (both directions, ~5.7 MB on the wire): {summary['bulk_phase_s']:.2f} s")
    print("\n| exchange | p50 | p99 | max |\n|---|---:|---:|---:|")
    for name, v in summary["latency"].items():
        print(
            f"| {name} ({v['req']} B -> {v['rep']} B) | {v['p50'] / 1e6:.2f} ms | "
            f"{v['p99'] / 1e6:.2f} ms | {v['max'] / 1e6:.2f} ms |"
        )


if __name__ == "__main__":
    if sys.argv[1] == "remote":
        remote(sys.argv[2])
    else:
        local(sys.argv[2], sys.argv[3:])
