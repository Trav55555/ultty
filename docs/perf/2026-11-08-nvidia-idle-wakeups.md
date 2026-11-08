# Idle wakeups on NVIDIA (2026-11-08)

**Claim class:** live-realgpu investigation. **Hardware:** AWS g4dn.xlarge, Tesla T4,
NVIDIA 580.178.04, Ubuntu 26.04, headless Xorg (virtual screen), openbox.
**Build:** main at 1a01273, `cursor_blink = false`, window focused.

## Question

The comparison runs reported ~114 context switches/s for an idle Ultty on the T4,
while the idle-footprint work (docs/perf/2026-11-07-idle-footprint.md) measured 0
under lavapipe. Ghostty on the same box: ~15/s.

## Findings

- Per-thread `/proc/<pid>/task/*/status` over 30 s: one thread accounts for
  99.6 of 100.1 switches/s; Ultty's UI thread is not it. `strace` on the UI
  thread shows a single `epoll_pwait` in 5 s, and no Ultty log lines are written.
- The busy thread waits on a condition variable with a 10 ms timeout, forever
  (`futex(... FUTEX_WAIT_BITSET ...)` = ETIMEDOUT every 10.1 ms). Its stack is
  entirely inside `libnvidia-glcore.so.580.178.04` (`pthread_cond_timedwait`
  called from the driver's own thread entry point).
- It does not depend on how Ultty presents: AutoVsync, Immediate, FifoRelaxed
  and `desired_maximum_frame_latency = 1` all give 113 switches/s. Mailbox is
  not offered by the driver here. (Tested with a throwaway build reading the
  present mode from an environment variable; not merged.)
- Idle CPU stays low: 0.17 CPU-s/min for Ultty against 0.30 for Ghostty in the
  same run.

## Conclusion

The remaining idle wakeups on NVIDIA come from a polling thread inside the
NVIDIA Vulkan driver that exists while Ultty holds a Vulkan device and
swapchain. Ultty's own threads are idle. Nothing in Ultty's present path
changes it; avoiding it would mean not using Vulkan on NVIDIA (the GL fallback
could not create a surface on this headless setup, so that was not measured)
or releasing the swapchain while idle, neither of which is worth its cost for
~0.2 CPU-s/min.

Comparison reports should read `idle.wakeups` on NVIDIA with this in mind and
prefer `idle.cpu`.
