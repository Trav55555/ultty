# Accessibility adapter: startup and idle cost (2026-11-08)

ADR 0021 slice 3 adds the AccessKit adapter (`accesskit_winit` 0.34.2,
`accesskit_unix` 0.24.1 on zbus 5 with async-io) and two checks for it:
the D-Bus connection leaves the startup path if it costs over 1 ms, and the
adapter becomes opt-in if it adds idle wakeups. The first holds; the second
does not, so `[behavior] accessibility` defaults to `false`.

## Method

Release builds of `main` (a9d8127) and the slice, on the development
workstation: Xvfb with lavapipe, `/bin/sh` as the shell, a fresh config
directory with `cursor_blink = false`. The machine was shared (load average
around 25 on 12 cores), so startup times move by about 10 ms between runs;
idle switch counts do not.

Three session-bus setups: **bus**, the desktop's session bus (accessibility
not enabled on it, the common case); **unset**, `DBUS_SESSION_BUS_ADDRESS`
unset, which zbus resolves to `$XDG_RUNTIME_DIR/bus`, so the same bus;
**dead**, the address pointing at a socket that does not exist, which is
what a headless CI runner without a session bus amounts to.

- **Startup**: `RUST_LOG=ultty::startup=debug`, milliseconds from exec,
  median of 9 launches, builds and setups interleaved.
- **Idle**: voluntary and involuntary context switches summed over
  `/proc/<pid>/task/*/status`, over 10 s after 5 s settle, with the thread
  list.

## Startup

Exec to "drawing the first frame", median of 9 (ms):

| build | bus | unset | dead |
|---|---:|---:|---:|
| main | 146.2 | 129.4 | 130.6 |
| slice, `accessibility = true` | 131.9 | 137.9 | 133.1 |
| main (second run) | 125.3 | 123.9 | 124.0 |
| slice, default (`false`) | 120.2 | 124.0 | 133.0 |

No difference outside the run-to-run noise. The new stage, "accessibility
adapter ready, window shown", comes 0.08 ms (median; at most 1.0 ms) after
"window created" in every setup: the adapter only spawns its thread and
queues a message, and that thread connects to the bus while the GPU comes
up. A missing bus neither fails nor delays startup; the thread finds no bus
and exits.

## Idle

| build | switches/s (vol + invol) | threads |
|---|---:|---:|
| main, any setup | 0.00 | 37 |
| slice, default (`false`), any setup | 0.00 | 37 |
| slice, `accessibility = true`, bus or unset | 2.00 + 0.0-0.1 | 40 |
| slice, `accessibility = true`, dead | 2.00 | 38 |

The new threads are the adapter's executor thread, `async-io` (the zbus
reactor, asleep in `epoll_wait`) and `blocking-1`. All the wakeups are
`blocking-1`: zbus connects its Unix socket through `blocking::unblock`, and
a `blocking` 1.7 pool thread with no work waits on a condition variable with
a 500 ms timeout and only exits when the pool holds more threads than its
limit, so the one thread it started wakes twice a second for the life of the
process. It happens with no bus as well, since the connect is attempted.

## Outcome

- The D-Bus connection is already off the startup path; nothing to move.
- The adapter adds idle wakeups, so per the ADR it is opt-in
  (`accessibility = false` by default) until this is fixed upstream (zbus
  or `blocking`), or Ultty drives zbus another way (its `tokio` feature with
  a runtime Ultty does not run today).
- zbus is not new to Ultty: notify-rust already pulls it in with async-io,
  so switching zbus to tokio changes desktop notifications too. Sending a
  notification probably starts the same `blocking` thread on `main` today,
  which would then wake twice a second from the first notification on;
  that is unmeasured.

With `accessibility = true` on a private session bus with
`at-spi-bus-launcher` and `org.a11y.Status.IsEnabled` set, AT-SPI lists an
`ultty` application with one `frame` named after the window title; the name
follows the pane's title, and turning `IsEnabled` off and on deactivates and
reactivates the adapter.

## Slice 4: the terminal tree

Same method, release build of the slice-4 branch, `accessibility = true`,
a private session bus with `at-spi-bus-launcher`. **Inactive** is
`IsEnabled` false; **active** is `IsEnabled` true after an AT-SPI client
read the terminal's text once. 10 s idle after 5 s settle, two runs each:

| setup | switches/s (vol + invol) | threads | trees sent while idle |
|---|---:|---:|---:|
| inactive | 2.00 + 0 | 36 | 0 |
| active, still screen | 2.00 + 0 | 36 | 0 |

The 2/s is still `blocking-1` (above); an active adapter with a still
screen adds no wakeups, because a rebuild is scheduled only after a
change.

Flooding the focused pane (a shell loop echoing 200 000 lines, about
1.4 s of output) while active sent 24 trees, about 17 per second; the
closest two were 47 ms apart in the log, which timestamps after the
build, against the 50 ms coalescing interval. Building a 44-node tree
for an 88x39 screen (snapshot under the pane's lock plus the build) took
a median of 93 us and at most 2.8 ms under the flood.
