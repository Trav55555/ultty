# Workstation Wayland latency, four terminals (D2, 2026-11-25)

Plan item D2 asked for keystroke-to-pixel latency idle and under output flood
on both machines. The T4 was measured on X11 and Wayland on 2026-11-08,
2026-11-11 and 2026-11-19. This is the workstation, on Wayland, with all four
terminals.

## Environment

- Workstation: AMD Radeon RX 7900 XT (RADV NAVI31), kernel 7.0.0-43, 12 cores.
- Private headless sway 1.11 rendering on the GPU's render node
  (`WLR_RENDER_DRM_DEVICE=/dev/dri/renderD128`, `WLR_RENDERER=gles2`, 60 Hz),
  so the desktop session is not involved.
- Ultty `cf2b6f3a6478`, a default release build. It includes the flood-latency
  fix (#148) and the Wayland startup fix (#238).
- Ghostty 1.4.0, Alacritty 0.16.1, GNOME Terminal 3.58.0 (VTE 0.84.0).
- 120x40 cells, DejaVu Sans Mono 11, padding 0. Claim class `live-realgpu`.

## Method

`scripts/bench/compare.sh --session wayland` with all four terminals, run as
three separate single rounds. Each round started only after five minutes of
1-minute load under 6 with no compiles running. A round was discarded if the
load went above 10 while it ran, or if any measurement failed.

The machine is shared with other agent sessions. Five rounds were started, and
two were discarded for load spikes (peaks 15.8 and 13.2). The three kept rounds
peaked at 6.7 to 7.0, with median load 5.2 to 5.3. Each round gives 200
keypresses per terminal and case, so 600 per row. Terminal order is shuffled
within a round. The raw samples are not committed.

## Keypress to pixel (ms, n=600 each)

| Case | Ultty | Alacritty | GNOME Terminal | Ghostty |
|---|---|---|---|---|
| Idle, median / p99 | 1.04 / 1.37 | 0.61 / 0.85 | 1.00 / 1.35 | 17.2 / 17.8 |
| Under 10 MiB/s output | 24.7 / 32.2 | 24.7 / 32.3 | 24.6 / 32.1 | 25.9 / 33.5 |
| Under maximum output | 27.0 / 41.5 | 30.8 / 49.4 | 48.5 / 144 | 31.8 / 43.8 |

- **Idle:** all four are well under a 16.7 ms frame except Ghostty, which waits
  for the next frame. Alacritty is 0.4 ms faster than Ultty at the median; Ultty
  and GNOME Terminal are level.
- **10 MiB/s of output:** the four are level. The ~25 ms is the compositor's
  frame pacing under a busy client, not the terminals.
- **Maximum output:** Ultty is the fastest, with the lowest median and p99. The
  comparative report finds Alacritty, Ghostty and GNOME Terminal all slower.
  Before #148, Ultty's median here was 225 ms in an unpublished 2026-11-13
  run on this machine; now it is 27 ms.

## Other rows from the same run (medians of three)

| | Ultty | Alacritty | GNOME Terminal | Ghostty |
|---|---:|---:|---:|---:|
| Exec to titled window | 91.7 ms | 91.5 ms | 661 ms | 474 ms |
| ASCII throughput | 252 MiB/s | 67.4 MiB/s | 99.5 MiB/s | 238 MiB/s |
| RSS after the feed | 77.6 MiB | 126 MiB | 59.4 MiB | 178 MiB |

No measurement failed in the kept rounds. All twelve Ultty launches started
cleanly. Before #238, three of twelve exited at startup with a Wayland
explicit-sync protocol error (#237).

## Command

```bash
env -u WAYLAND_DISPLAY WLR_RENDER_DRM_DEVICE=/dev/dri/renderD128 \
  scripts/bench/compare.sh --session wayland \
    --terminals ultty,ghostty,alacritty,gnome-terminal \
    --rounds 1 --idle-secs 0 --tabs "" --size-mib 4 --repeat 1 --corpora ascii \
    --ultty-dir target/release --bench-dir target/release --out <dir>
```

Each run is one round. The three kept rounds' `results.jsonl` are combined
(renumbering `block`) and analysed with `scripts/bench/analyze.py`.
