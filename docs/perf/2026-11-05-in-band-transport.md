# In-band transport prototype benchmark (2026-11-05)

This benchmark answers the open transport question in
[ADR 0018](../adr/0018-in-band-native-transport.md): what does it cost to
carry native frames in the PTY stream as `ESC _ U <base64url> ESC \`
instead of over the per-tab Unix socket?

## Result

On real app traffic, the time cost is negligible. The byte cost is high and
fixed. Bulk payloads are where the socket is clearly better.

- **Bytes: +35%.** Base64 adds a third and the APC wrapper adds 5 bytes, so
  this is the same for every app. It cannot be tuned below ~33% without an
  encoding that is no longer plain ASCII, and plain ASCII is what lets frames
  survive recorders and ssh.
- **Terminal CPU: 2.0-2.5× the socket path, measured in microseconds.**
  A whole recorded session costs 4-9 µs in-band against 2-4 µs over the
  socket.
- **Round-trip latency: +6 to +14 µs at p50, ≤ 290 µs at p99.9.** That
  is under 2% of a 60 Hz frame (16.7 ms) even at the tail.
- **Bulk: about 330 MiB/s over the PTY, against about 7 GiB/s over the
  socket.** A 1 MiB image takes 3 ms in-band and 0.13 ms over the socket.
  That is fine for occasional images and wrong for video-rate updates. It
  supports keeping the socket as a local fast path, as the ADR proposes.

In [ADR 0018](../adr/0018-in-band-native-transport.md) I proposed accepting
in-band if the overhead stayed under ~10% for typical UI diffs. Measured in
bytes, that criterion fails by construction (+35%). Measured in time, the
cost that users feel, it passes by two orders of magnitude. The
recommendation below goes by time.

## Method

- **Traffic.** Five example apps (`counter-app`, `todo-app`, `form-app`,
  `chat`, `dashboard`) ran in a live Ultty tab with scripted input. A proxy
  between each app and `ULTTY_SOCKET` recorded every chunk in both
  directions. App-to-terminal frames are grouped into whole
  `BeginFrame..EndFrame` updates, each sent as one APC. Over this corpus,
  first renders are 1.1-2.2 KB, later updates are 24-260 B, and the largest
  terminal reply (`LayoutComputed`) is 721 B.
- **Stress cases.** Two synthetic batches stress size: a 2,000-region
  scene (290 KiB) and a 512×512 RGBA image (1 MiB).
- **Benchmark.** `crates/ultty/examples/inband_transport.rs` (release
  build).
  - *Terminal CPU* covers the VT parser running over the APC bytes on its
    existing discard path, plus a separate extractor, base64 decode and
    frame decode. The real handler would extract inside the parser's
    existing pass, so this slightly overcounts.
  - *Latency* is a two-thread ping-pong: a socketpair for the socket path,
    and a raw-mode `openpty` pair for in-band.
- **Machine.** Linux 7.0, same machine as
  [2026-11-05-io-thread.md](2026-11-05-io-thread.md). Two runs agreed
  within 10%, except the p99.9 tails, which vary between runs.

Reproduce:

```sh
cargo build --release -p ultty -p counter-app -p todo-app -p form-app -p chat -p dashboard
cd scripts/inband-bench
./capture.sh && python3 corpus.py
cargo run --release -p ultty --example inband_transport -- corpus.bin
python3 tmux_passthrough.py
python3 ssh_roundtrip.py local corpus.bin ssh -i <key> <user>@<host>
```

## Numbers (second run)

### Bytes, app to terminal

| app | updates | raw | in-band | overhead |
|---|---:|---:|---:|---:|
| counter-app | 10 | 1,868 | 2,543 | +36% |
| todo-app | 9 | 2,418 | 3,272 | +35% |
| form-app | 8 | 2,835 | 3,823 | +35% |
| chat | 5 | 1,160 | 1,573 | +36% |
| dashboard | 5 | 2,323 | 3,124 | +34% |

### Terminal-side CPU per batch

| batch | raw bytes | socket | in-band | of which VT parse |
|---|---:|---:|---:|---:|
| counter-app session | 1,868 | 2.7 µs | 6.2 µs | 2.1 µs |
| todo-app session | 2,418 | 3.2 µs | 7.8 µs | 2.9 µs |
| form-app session | 2,835 | 4.0 µs | 9.1 µs | 3.1 µs |
| chat session | 1,160 | 1.7 µs | 3.7 µs | 1.3 µs |
| dashboard session | 2,323 | 3.2 µs | 7.3 µs | 2.6 µs |
| 2,000-region scene | 296,917 | 322 µs | 847 µs | 328 µs |
| 512×512 RGBA image | 1,048,603 | 25 µs | 1.86 ms | 1.15 ms |

### Round trip, app update to terminal reply

| exchange | transport | p50 | p99 | p99.9 |
|---|---|---:|---:|---:|
| median (24 B → 28 B) | Unix socket | 6.6 µs | 14.2 µs | 47.1 µs |
| median (24 B → 28 B) | PTY + APC | 12.3 µs | 22.0 µs | 119 µs |
| largest (2,178 B → 721 B) | Unix socket | 7.1 µs | 12.8 µs | 25.3 µs |
| largest (2,178 B → 721 B) | PTY + APC | 20.2 µs | 33.9 µs | 134 µs |

The first run's PTY p99.9 values were 261 and 286 µs. The other columns
agreed with the second run.

### Bulk, one message (best of 5)

| payload | direction | Unix socket | PTY + APC |
|---|---|---:|---:|
| 1.00 MiB image | app → terminal | 0.13 ms | 3.04 ms |
| 1.00 MiB image | terminal → app | 0.11 ms | 3.04 ms |
| 0.28 MiB scene | app → terminal | 0.05 ms | 0.86 ms |
| 0.28 MiB scene | terminal → app | 0.04 ms | 0.86 ms |

## Crossing intermediaries

The tests used tmux 3.6 and util-linux `script` 2.42. The outer terminal was
a PTY held by the test script (`scripts/inband-bench/tmux_passthrough.py`).

| case | result |
|---|---|
| `script` records an APC frame | Verbatim in the typescript |
| tmux, bare APC from the app, passthrough off or on | Dropped |
| tmux, DCS-wrapped APC, `allow-passthrough off` | Dropped |
| tmux, DCS-wrapped APC, `allow-passthrough on` | Delivered, up to 1,048,000 B; a 1,049,000 B frame is dropped (1 MiB cap) |
| tmux, APC written into the outer terminal's input | Delivered verbatim to the active pane |
| ssh to EC2 (see below) | All 114 frames verbatim, both directions, up to 1.4 MB unchunked |

Three things follow for the design:

1. **Send the probe bare and never DCS-wrap it.** tmux drops bare APC
   strings, so an app inside tmux gets no probe reply and falls back to ANSI.
   That is the correct outcome, because Ultty cannot know where a tmux pane
   sits inside its tab. The ADR's text said tmux never forwards replies to
   panes; it does, to the active pane, so the probe must be the safeguard.
2. **Chunk frames.** Split frames into APCs well under 1 MiB, for example
   64 KiB. This also bounds what the VT parser must buffer for one string.
3. **Images stay on the socket when it is available.** In-band is for
   reach, not bandwidth.

## Over a real ssh hop (2026-11-05)

`scripts/inband-bench/ssh_roundtrip.py` ran against a throwaway t4g.nano
(Amazon Linux 2023, us-east-1), which was torn down afterwards. The client
was a home connection; TCP connect round trip was 25-37 ms. The app side ran
on the instance with its tty in raw mode. The terminal side ran `ssh -tt`
on a local PTY, as Ultty runs a shell in a tab. Every frame carried a
SHA-256 of its data, and each side checked the other's.

- **Integrity, app to terminal: 54 of 54 frames verified.** These were all
  recorded app updates, 1 MiB in 64 KiB chunks, and one 1.4 MB frame.
- **Integrity, terminal to app: 60 of 60 frames verified.** These were all
  recorded replies, the same chunked 1 MiB, and one 1.4 MB frame.
- **Round trip (ms, two runs):**

| exchange | run 1 p50 / p99 | run 2 p50 / p99 |
|---|---:|---:|
| median real update (24 B -> 28 B), APC | 38.9 / 43.2 | 49.3 / 162.8 |
| largest real update (2,178 B -> 721 B), APC | 23.5 / 62.9 | 28.8 / 75.8 |
| raw 1 byte -> 1 byte, no framing | 39.7 / 57.5 | 40.6 / 42.6 |

The network sets these numbers. The framed and unframed cases swap places
between runs, and the same bulk transfer took 0.85 s in one run and 6.2 s in
the other. Against a 25-40 ms round trip, the 6-14 µs local cost of APC
framing cannot be measured. ssh carried every frame byte for byte.

## Side findings

- **Empty updates.** 11 of the 27 recorded `BeginFrame..EndFrame` pairs
  were empty: a 24-byte frame with nothing between begin and end. `ulttui`
  sends a frame even when its diff is empty. Skipping those is a cheap
  win on either transport.
- **sysmon stalls under the proxy.** Through the recording proxy, `sysmon`
  completed the handshake (`ClientHello`, `ServerHello`, `SessionReady`) and
  then sent nothing for 15 s. It renders when run directly. The cause is not
  yet known. The other five apps behave the same with and without the proxy.
