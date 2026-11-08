# Control plane latency, and background tabs (2026-11-08)

Plan item D6 asks for a local control request under 1 ms at p99, and for
the latency of event delivery to subscribers (A3). Both are measured by
[`scripts/bench/control-latency.py`](../../scripts/bench/control-latency.py)
against a running Ultty. Measuring them found that a background tab's
output redrew the window; that is fixed here.

All numbers are from the development workstation (Ryzen 5 7600), Xvfb
with lavapipe (Mesa's software Vulkan), a release build, two runs each.
Software rendering makes a frame cost about 14 ms (median of 336 frames,
`ULTTY_TRACE_PIPELINE=1`), far more than on a GPU, which matters below.

## Results

| Path | median | p99 | max |
|---|---:|---:|---:|
| request: connect, `ping` with the token, reply read to EOF | 0.05 ms | 0.12-0.14 ms | 0.2-0.6 ms |
| `ulttyctl ping`, process start to exit | 0.83-0.87 ms | 1.4-2.0 ms | 2.2-4.3 ms |
| event: `send_text` to a background `cat`, its echoed OSC 9 back as a `notification` event | 0.10 ms | 0.41-0.49 ms | 0.9-1.1 ms |
| event+draw: the same on screen, an OSC 2 title, so each event also draws a frame | 0.16-0.17 ms | 0.9-1.4 ms | 1.6-3.1 ms |

The event rows include the whole loop: the request, the write to the PTY,
`cat` reading and echoing it, the I/O thread parsing it, the UI thread
collecting the event and writing it to the subscriber.

A process-per-request client such as `ulttyctl` pays mostly for its own
start; a long-running client (the MCP server, an agent holding a
subscription) pays the request row.

## The first measurement was wrong

The first probe rang the bell (BEL echoed by `cat`) and saw 27-30 ms
median, 47 ms p99, with the pane on screen and 11 ms median with it in a
background tab. Two causes, neither in the event path:

- A bell starts a 100 ms visual flash. Ringing every few milliseconds kept
  the window redrawing at 60 fps, and the UI thread, which also answers
  control requests and publishes events, spent most of each 16 ms in a
  14 ms software frame. The probe now uses events that do not animate.
- Output in a background tab set `needs_redraw`, so the hidden pane's echo
  drew a frame too. That is a real cost, below.

## Background tabs no longer redraw the window

A pane in a background tab printing 100 lines a second for 5 s (an agent
streaming in another tab), measured as Ultty's `utime + stime` and frames
drawn over the 5 s:

| | CPU | frames |
|---|---:|---:|
| before | 0.24 s | 272 |
| after | 0.04-0.05 s | 9 (the cursor blink) |

Output now asks for a frame only when its pane is in the active tab
(`TerminalManager::is_on_screen`). Switching tabs already redraws, and the
hidden pane's grid is drawn as it stands then.

## Not measured here

- The same table on a GPU (the T4), where a frame costs about 1 ms; the
  event+draw row should approach the event row.
- Event delivery under heavy output in the same pane: the UI collects
  events once per pass, so a pass that draws a frame delays them by up to
  that frame.
