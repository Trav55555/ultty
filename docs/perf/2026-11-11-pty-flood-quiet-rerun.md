# `pty_flood` on a quiet c7a after the frame fix, and the condvar wake (2026-11-11)

Measurement. The [frame quantum note](2026-11-11-pty-flood-frame-quantum.md)
showed that the old `pty_flood` rounded every run up to a 16 ms frame, so
its quiet-c7a figure of about 219 MiB/s for ascii was the bench's, not
Ultty's. This note measures the fixed bench (#107) on a quiet c7a.2xlarge,
and the effect of removing the per-chunk `notify_all` futex that the same
investigation found.

**Verdict.**

- **Main, with the fixed bench:** ascii 120x40 runs at **261.9 MiB/s**
  (219 was an artefact; +19.6%), ascii 300x75 at 255.5, cjk at 92.4 / 91.3
  and sgr-dense at 88.7 / 87.7. Every earlier published `pty_flood` figure
  from the frame-quantised bench is a lower bound, not a ceiling.
- **The futex change** (`pty-wake-futex`) is neutral-or-better everywhere:
  +1.2% to +1.8% on all six cases, every 95% interval above 1.000. It
  removes about 91% of futex system calls on a local flood. It is a small
  win, not the large one that the 4–8 ms guess in the frame note suggested.

## Change

`Shared::changed` is a `Condvar`. Its only waiter is `EventLoop::wait`,
which the app does not call. Callers of `wait` are `EventLoop::run`, the
`app_compat` integration test, the `pty_throughput` example and the
event-loop unit tests; the `pty_flood` bench waits on its own condvar, and
`ulttyctl` and the automation paths do not use `EventLoop::wait`. Yet
`WakeThrottle::output` called `changed.notify_all()` for every throttled
chunk, and std's futex `Condvar` makes a `futex_wake` system call even with
no waiter.

`Shared` now counts waiters (registered under the state lock, before
`wait_timeout_while` rechecks `ready`) and `notify_waiters` skips the
`notify_all` at zero. A waiter that registers after the load has already
seen the state change under the lock, so it does not sleep through it.
`EventLoop::wait` returning promptly on output with a long timeout is
covered by `wait_wakes_on_output_without_running_out_its_timeout`.

## Futex calls, local

`strace -f -c` over one criterion run of `pty_flood` ascii, both sizes, same
machine, same `--warm-up-time 1 --measurement-time 3` (the shared
workstation was loaded at 17–26, and strace slows every run, so the times
mean nothing; the counts are what is compared):

| arm | `futex` | `read` | futex per read | `epoll_pwait` |
|---|---:|---:|---:|---:|
| `origin/main` (`732be1f`) | 205,148 | 194,142 | 1.06 | 1,212 |
| `pty-wake-futex` | 18,887 | 193,303 | 0.10 | 1,429 |

The reads match, which fixes the number of floods at about 24 (about 8,200
reads each). So the futex count falls from about 8,700 to about 800 per
flood. What is left comes from criterion's rayon pool, state-lock
contention with the UI thread, and the throttled wakes that still happen.

## The quiet c7a run

### Method

- AWS c7a.2xlarge (AMD EPYC 9R14, 8 vCPUs), us-east-1, on demand, Ubuntu
  26.04, kernel 7.0.0-1016-aws, rustc 1.99.1 (the AMI's toolchain), no
  cpufreq interface, not pinned. One instance, about 80 minutes from launch to
  terminate.
- Quiet: snapd, unattended-upgrades and the timers that could fire were
  stopped before the run; nothing else ran. The 1-minute load was 1.29 to
  2.95 during the batches, which is the harness itself.
- Two bench binaries built on the instance with `cargo bench --locked -p
  ultty --bench pty_flood --no-run` from `git archive` of `origin/main`
  (`732be1f`, arm A) and `pty-wake-futex` (`9fedaa7`, arm B), sharing one
  target directory. The bench file is identical in both.
- `scripts/bench/pty-flood-ab.sh`: every arm once per case per round, the
  arm order alternating by round and by batch, criterion's 10 samples per
  run (`--warm-up-time 1 --measurement-time 3`), and the run's median
  sample is the number. One discarded trial round, then three batches of 15
  rounds, 45 in all, over ascii, cjk and sgr-dense at both sizes (the
  other two corpora were not run).
- `scripts/bench/pty-flood-analyze.py` computes the tables below. Paired
  per-round ratios B / A, 95% percentile bootstrap over rounds (10,000
  resamples). Raw data is `data/2026-11-11-pty-flood-quiet-rerun/runs.tsv`
  (the load column was recovered from the run log, because the first
  version of the script dropped it; the script is fixed). Criterion's raw
  per-sample times are kept off-repo.

### Results

| case | arm | median MiB/s | IQR / median | range |
|---|---|---:|---:|---:|
| ascii 120x40 | `main` | 261.9 | 0.2% | 255.6–262.5 |
| ascii 120x40 | wake fix | 265.4 | 0.3% | 262.3–266.3 |
| ascii 300x75 | `main` | 255.5 | 0.4% | 243.6–257.1 |
| ascii 300x75 | wake fix | 260.0 | 0.4% | 251.5–261.4 |
| cjk 120x40 | `main` | 92.4 | 0.3% | 89.7–92.7 |
| cjk 120x40 | wake fix | 93.9 | 0.2% | 91.8–94.2 |
| cjk 300x75 | `main` | 91.3 | 0.2% | 89.7–91.6 |
| cjk 300x75 | wake fix | 92.7 | 0.8% | 86.6–93.0 |
| sgr-dense 120x40 | `main` | 88.7 | 0.3% | 87.7–89.0 |
| sgr-dense 120x40 | wake fix | 89.7 | 0.3% | 86.9–90.0 |
| sgr-dense 300x75 | `main` | 87.7 | 0.3% | 85.8–88.0 |
| sgr-dense 300x75 | wake fix | 89.0 | 0.2% | 88.3–89.2 |

| case | wake fix / `main` | 95% CI | IQR of the ratio | batches 1, 2, 3 |
|---|---:|---|---|---|
| ascii 120x40 | 1.014 | 1.013–1.015 | 1.011–1.016 | 1.014, 1.013, 1.014 |
| ascii 300x75 | 1.018 | 1.016–1.019 | 1.014–1.020 | 1.017, 1.018, 1.019 |
| cjk 120x40 | 1.016 | 1.015–1.016 | 1.014–1.018 | 1.016, 1.015, 1.016 |
| cjk 300x75 | 1.015 | 1.013–1.017 | 1.010–1.017 | 1.016, 1.015, 1.013 |
| sgr-dense 120x40 | 1.012 | 1.011–1.013 | 1.010–1.013 | 1.012, 1.013, 1.011 |
| sgr-dense 300x75 | 1.014 | 1.014–1.015 | 1.012–1.016 | 1.015, 1.015, 1.014 |

The gain is 1.2–1.8% in every case and every batch. In time that is about
1.7 ms on a 122 ms ascii run and 4–6 ms on the 350–370 ms cjk and sgr-dense
runs, the same order as the frame note's 4–8 ms guess for a flood. Against the numbers on the old bench: `main` ascii 120x40 went from 219.3
to 261.9 MiB/s, ascii 300x75 from 218.1 to 255.5, cjk from 86.2 / 85.8 to
92.4 / 91.3, and sgr-dense from 82.5 / 82.2 to 88.7 / 87.7. The frame note
guessed 220 to 250 for ascii; it was a little higher.

### The runs are no longer on the 16 ms grid

For cjk and sgr-dense, criterion's samples are single floods, so each run's
time can be reduced modulo 16 ms (450 runs per row, from 45 rounds of 10
samples):

| case | arm | residue mod 16 ms: min / median / max (ms) | 1 ms bins of 16 hit |
|---|---|---|---:|
| cjk 120x40 | `main` | 0.1 / 9.5 / 15.8 | 16 |
| cjk 120x40 | wake fix | 0.0 / 4.5 / 15.9 | 15 |
| cjk 300x75 | `main` | 0.0 / 13.9 / 15.8 | 15 |
| cjk 300x75 | wake fix | 0.0 / 8.9 / 15.9 | 16 |
| sgr-dense 120x40 | `main` | 0.6 / 8.9 / 15.5 | 13 |
| sgr-dense 120x40 | wake fix | 1.0 / 4.6 / 15.9 | 16 |
| sgr-dense 300x75 | `main` | 0.4 / 12.8 / 15.7 | 13 |
| sgr-dense 300x75 | wake fix | 0.8 / 7.7 / 13.0 | 12 |

Before, every remainder fell between 1 and 7.1 ms. Now they cover the whole
frame. The ascii floods are about 120 ms, so criterion batches three of
them per sample and the sample is a mean, which cannot be reduced modulo a
frame; ascii's quantisation is therefore not tested directly. Its means
(121.9 ms for `main` 120x40, which is 7 frames and 9.9 ms) are not at the
old 9 frames and 1–7 ms, and the cjk and sgr-dense evidence shows the bench
is continuous now.

## What this does not show

- One instance and one run of 45 rounds, not repeated on another instance.
- Three batches of 15 rounds, as the earlier runs; unicode-mixed and
  cursor-redraw were not run on the c7a.
- The strace counts are from the loaded workstation, not the c7a.
- The ascii frame residue, as above.
- No GPU, window or live-terminal effect. The app does not wait on the
  condvar, so for the app the change only removes the I/O thread's system
  calls; whether a visible latency or CPU change follows was not measured.
