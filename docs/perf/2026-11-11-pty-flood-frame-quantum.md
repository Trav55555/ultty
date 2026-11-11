# `pty_flood`'s ascii ceiling was the bench's frame sleep (2026-11-11)

Investigation. [Page storage's results](2026-11-11-page-storage.md) found
`pty_flood` ascii stuck at about 219 MiB/s on a quiet c7a.2xlarge: 219.1
at slice 4, 219.3 after slice 5 cut vt/ascii instructions by another 30%,
and 218.1 at 300x75. This note finds the cause and fixes the bench.

**Verdict.** The ceiling was the bench, not Ultty. The bench thread played
the UI with a blind `thread::sleep` until the next frame, so it only saw
the child's exit at a frame boundary. Every run's time was a whole number
of 16 ms frames plus a small constant, and ascii sat at 9 frames. The bench
now waits for the next frame on the waker, as the app's event loop does
(`ControlFlow::WaitUntil` plus waker events), and sees the exit when it is
reported. The ceiling is gone locally. The quiet-machine numbers for the
new bench are not measured yet.

## The evidence

### Every published `pty_flood` time sits on the 16 ms grid

32 MiB divided by each quiet-c7a median from the page storage note, and
by the workstation "PTY ceiling" in the slice 5 note:

| case | MiB/s | time | as frames |
|---|---:|---:|---|
| ascii 120x40, `main` | 219.3 | 145.9 ms | 9 x 16 + 1.9 |
| ascii 120x40, slice 4 | 219.1 | 146.1 ms | 9 x 16 + 2.1 |
| ascii 300x75, `main` | 218.1 | 146.7 ms | 9 x 16 + 2.7 |
| ascii 120x40, before slice 3 | 164.7 | 194.3 ms | 12 x 16 + 2.3 |
| ascii 300x75, before slice 3 | 153.1 | 209.0 ms | 13 x 16 + 1.0 |
| cjk 120x40, `main` | 86.2 | 371.2 ms | 23 x 16 + 3.2 |
| cjk 300x75, `main` | 85.8 | 373.0 ms | 23 x 16 + 5.0 |
| cjk 120x40, before slice 3 | 60.1 | 532.4 ms | 33 x 16 + 4.4 |
| sgr-dense 120x40, `main` | 82.5 | 387.9 ms | 24 x 16 + 3.9 |
| sgr-dense 120x40, before slice 3 | 85.3 | 375.1 ms | 23 x 16 + 7.1 |
| ascii 120x40, workstation (slice 5) | 240.0 | 133.3 ms | 8 x 16 + 5.3 |

Every remainder falls between 1 and 7.1 ms, out of a possible 0 to 16.
That also explains the "IQR 0.0%" arms and the ascii ratio of exactly
1.332 in every batch. 12 frames over 9 frames, with the same constant
added to each, gives that ratio. Slices 4 and 5 both finished somewhere
in the ninth frame, so the bench could not tell them apart.

### The mechanism

During a flood the I/O thread wakes the UI every 4 ms
(`OUTPUT_WAKE_INTERVAL`), so the bench always had a `Redraw` and always
slept out the rest of the frame before it copied rows. When `dd` exited
and the I/O thread closed the PTY, its wake landed during that sleep. The
bench saw it only after waking at the frame boundary and copying the rows.
The app does not behave like this: it waits with `WaitUntil` the next
frame deadline, and a waker event ends that wait early.

### Local reproduction (shared workstation, 12 threads, load 12–22)

A throwaway build of the bench took a `PACING` switch, so both loops ran
from one binary. The old loop is `PACING=sleep`, identical to `main`. The
new loop is `PACING=prompt`, identical to this branch. The build printed
every run's time. Both arms alternated by round, 16 rounds in all, with
criterion's samples at `--warm-up-time 1 --measurement-time 3`, ascii at
both sizes.

| loop | runs | fastest five (ms) | runs under 128 ms | best MiB/s |
|---|---:|---|---:|---:|
| old (blind sleep) | 550 | 129.6 129.7 129.7 129.8 129.8 | 0 | 247 |
| new (waits on the waker) | 512 | 108.3 108.8 111.0 111.0 112.5 | 37 | 295 |

The old loop's floor is pinned at 8 x 16 + 1.7 ms, the same grid one frame
lower than on the c7a. The new loop goes 21 ms under it. Medians on this
machine (122 against 132 MiB/s) mostly measure the other load on the box,
so the fastest runs are the useful statistic here.

Removing VT work does not move the old loop off the grid. A second
throwaway switch made `parse_into` return before parsing. With no VT work
at all, the old loop's best was 113.7 ms (7 x 16 + 1.7). The new loop's
best was 88.6 ms (361 MiB/s), against 113.8 ms with parsing in the same
alternated rounds. So locally, about 25 ms of a 114 ms best run is VT work.
The rest is spawn, PTY transfer and the event loop.

## The I/O path outside the VT core

`strace -f -c` over one criterion run (11 floods of 32 MiB, old loop):

- **Reads.** About 8,200 PTY reads per flood, almost all 4,095 bytes. The
  kernel's n_tty buffer is 4 KiB, so `READ_CHUNK` (64 KiB) never fills,
  and a larger buffer would change nothing. A Python reader on the same
  PTY setup sees the same 4,095-byte reads.
- **EAGAIN and polls.** About 48 EAGAIN reads and 24 `epoll_pwait` calls
  per flood. The I/O thread almost never runs dry, so it is the busy side
  and `dd` keeps up with it.
- **Futex calls.** About 8,400 per flood, one for each chunk. Most come from
  `WakeThrottle::output`'s `shared.changed.notify_all()`, which runs for
  every chunk whose wake is throttled. The std futex `Condvar` makes a
  `futex_wake` system call on every `notify_all`, even with no waiter, and
  only `EventLoop::wait` waits on `changed`; the app does not. At an
  assumed 0.5–1 µs a call (not measured here), that would be 4–8 ms of a
  ~100 ms flood. This is the next lead outside the VT core, and it is not
  changed here.
- **Locks.** Two lock acquisitions per chunk (parse, then
  `flush_outbound`). These are uncontended apart from the UI's per-frame
  row copy, so they cost far less than the futex calls.
- **`sched_yield`.** The ~1,170 `sched_yield` calls per flood come from
  criterion's rayon pool (`WorkerThread::wait_until_cold`), not from the
  flood path.

`perf` is not usable on the workstation (`perf_event_paranoid` is 4), so
there is no cycle profile here.

## Not measured

- The new bench on a quiet c7a.2xlarge. Its ascii number there will be
  higher than 219 by an amount this note cannot predict. The true time
  was somewhere in the ninth frame, so 128 to 144 ms plus the constant,
  which is roughly 220 to 250 MiB/s. Earlier `pty_flood` ratios between
  arms that sit in different frames hold only to about a frame (11% at
  ascii's 146 ms, 4% at cjk's 371 ms).
- The futex fix.
