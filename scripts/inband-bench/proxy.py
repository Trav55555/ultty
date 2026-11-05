"""Forward a native-protocol Unix socket and log every chunk with a timestamp.

Usage: proxy.py <listen-path> <upstream-path> <trace-out>
Record format: dir u8 (0 = app->terminal, 1 = terminal->app), t_ns u64, len u32, bytes.
"""
import os, socket, struct, sys, threading, time

listen_path, upstream_path, out_path = sys.argv[1:4]
out = open(out_path, "wb")
lock = threading.Lock()

def pump(src, dst, direction):
    while True:
        data = src.recv(65536)
        if not data:
            break
        with lock:
            out.write(struct.pack("<BQI", direction, time.monotonic_ns(), len(data)) + data)
            out.flush()
        dst.sendall(data)
    try:
        dst.shutdown(socket.SHUT_WR)
    except OSError:
        pass

srv = socket.socket(socket.AF_UNIX)
srv.bind(listen_path)
srv.listen(1)
app, _ = srv.accept()
term = socket.socket(socket.AF_UNIX)
term.connect(upstream_path)
t = threading.Thread(target=pump, args=(term, app, 1), daemon=True)
t.start()
pump(app, term, 0)
t.join(2)
