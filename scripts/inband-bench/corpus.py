"""Group traced frames into updates and write corpus.bin.

App->terminal: BeginFrame..EndFrame is one update; frames outside a pair
(hello, append, etc.) are their own update. Terminal->app: one message each.
Record: dir u8, app-index u8, len u32, bytes.
"""
import struct, sys, statistics as st
from frames import frames
apps = ["counter-app", "todo-app", "form-app", "chat", "dashboard"]
out = open("corpus.bin", "wb")
for ai, app in enumerate(apps):
    f = frames(f"{app}.trace")
    updates, cur = [], None
    for _, ty, b in f[0]:
        if ty == 0x30: cur = bytearray(b)
        elif cur is not None:
            cur += b
            if ty == 0x31: updates.append(bytes(cur)); cur = None
        else: updates.append(b)
    for u in updates: out.write(struct.pack("<BBI", 0, ai, len(u)) + u)
    for _, _, b in f[1]: out.write(struct.pack("<BBI", 1, ai, len(b)) + b)
    sz = sorted(len(u) for u in updates)
    print(f"{app:12} updates={len(sz):3} median={int(st.median(sz)):5} p90={sz[int(len(sz)*.9)]:5} max={sz[-1]:5}")
