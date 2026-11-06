"""Split traces into protocol frames: frames.py <trace>... -> per-app stats."""
import struct, sys, statistics as st
def frames(path):
    streams = {0: bytearray(), 1: bytearray()}; out = {0: [], 1: []}
    data = open(path, "rb").read(); i = 0
    while i < len(data):
        d, t, n = struct.unpack_from("<BQI", data, i); i += 13
        streams[d] += data[i:i+n]; i += n
        s = streams[d]
        while len(s) >= 8:
            assert s[:2] == b"UT", (path, d, bytes(s[:8]))
            ln = struct.unpack_from("<I", s, 4)[0]
            if len(s) < 8 + ln: break
            out[d].append((t, struct.unpack_from("<H", s, 2)[0], bytes(s[:8+ln]))); del s[:8+ln]
    return out
if __name__ == "__main__":
    for p in sys.argv[1:]:
        f = frames(p)
        for d in (0, 1):
            sizes = [len(b) for _, _, b in f[d]]
            if sizes:
                print(f"{p.split('/')[-1]:22} {'app->term' if d==0 else 'term->app'} n={len(sizes):3} total={sum(sizes):6} median={int(st.median(sizes)):5} max={max(sizes):5}")
