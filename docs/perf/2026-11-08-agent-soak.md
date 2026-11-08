# Ten-hour agent-session soak (2026-11-08)

Plan item D5 asks for flat memory and a frame time p99 under 5 ms over a
ten-hour session shaped like an agent CLI. [`scripts/bench/soak.sh`](../../scripts/bench/soak.sh)
runs Ultty on Xvfb with two tabs of
[`scripts/bench/agent-stream.py`](../../scripts/bench/agent-stream.py)
(one on screen, one in the background): tokens streamed at 50 a second,
a status box redrawn every 80 ms in synchronized output, tool output dumps
of 200 to 3000 lines, and `/clear` now and then. The window is resized
every 5 minutes. Release build, Ryzen 5 7600, RX 7900 XT through RADV.

## Results

600 minutes, run 02:19 to 12:19.

| | |
|---|---|
| frame time, median of per-minute medians | 0.80 ms |
| frame time, median of per-minute p99s | 1.32 ms |
| minutes with p99 over 5 ms | 35 of 600; none after minute 115 |
| worst minute p99 | 26.7 ms (minute 17) |
| CPU | 2162 s, about 6% of one core |
| RSS, minutes 30 to 45 | 89.1 to 89.3 MiB |

Ultty ran the whole ten hours, and its frame time met the target once
the host settled (minutes 116 to 600: every minute's p99 under 5 ms,
typically 1.3 ms).

## Memory: inconclusive

At minute 47 RSS fell from 87 to 35 MiB and stayed at 33 to 34 MiB to the
end. Ultty did not free 50 MiB: the host was short of memory (its 10 GiB
of swap was full when checked after the run), and the kernel reclaimed
Ultty's pages. The slow minutes (32 of the 35) fall in the same window,
minutes 41 to 115.

A second run, recording swap, saw the same drop at minute 42 (77 to 48
MiB) with nothing swapped out. Most of what goes is file-backed: the
mapped libraries, the GPU driver chief among them, which the kernel drops
without swapping. At minute 137 of that run Ultty held 27 MiB of heap and
stacks (`RssAnon`) and 9.6 MiB of file pages. Either way RSS cannot show
a leak in this state: leaked heap is touched once and can then be swapped
out, and RSS moves with page cache pressure from other processes.

`soak.sh` now records `RssAnon` and `VmSwap`, and `soak-report.py` judges
their sum. The first run only shows memory flat for the 15 minutes
between warm-up and the reclaim, which is too short to judge. D5's memory
criterion needs a run measured this way.

## Before this run

The first trial of the soak found each resize leaving the process up to
100 MB larger: reflow rebuilt all of scrollback at once. Reflow now
streams line by line (peak heap for a 10 000-line resize 149 MB to
4.7 MB), and the `typical_10k_lines_120x40_resized` case in the memory
bench gates it. An 11-minute trial afterwards stayed at 80 to 85 MiB,
against 175 to 355 MiB before.
