# ADR 0027: Multiple Windows per Process

## Status

**Proposed** 2026-11-17. Nothing here is scheduled. Code references are to
`main` at 252c082.

## Context

### The gap

`docs/feature-parity.md` gap 9: "New window" starts a second Ultty
process. Ghostty and kitty run every window in one process. The quick
terminal (ADR 0028) needs a window that the running instance can show and
hide, which is easiest if one process can own more than one window.

### What Ultty has today

- **"New window" is a new process.** `MenuItem::NewWindow` in
  `crates/ultty/src/app/input.rs` runs `current_exe()` with no arguments,
  under the comment "One window per process: start another Ultty."
- **`App` is one window.** `App` (`crates/ultty/src/app/mod.rs`) holds one
  `window`, one `surface`, one `Renderer`, one optional AccessKit adapter,
  one `SessionRuntime`, one `ControlServer`, one `ConfigWatcher`, and all
  per-window UI state: tab bar, mouse and selection, IME preedit, copy
  mode, search, overlays, frame pacing and `needs_redraw`.
- **The winit handler ignores the window id.** `AppHandler::window_event`
  takes `_window_id` and passes every event to the one `App`. `AppHandler`
  creates the `App` on the first `resumed`.
- **GPU objects are per process.** `request_gpu` builds a
  `wgpu::Instance` and asks for an adapter compatible with the window's
  surface. `Renderer::new` (`renderer/mod.rs`) asks that adapter for a
  device and queue, held as `Arc<wgpu::Device>` and `Arc<wgpu::Queue>`.
- **Fonts and glyphs are per renderer.** `TextRenderer`
  (`renderer/text.rs`) owns a `FontSystem`, a `GlyphAtlas`, one set of
  `Metrics`, and shaping, run and cluster caches. `GlyphAtlas`
  (`renderer/atlas.rs`) is one RGBA texture that starts at 1024 square and
  grows to 2048, with its own `SwashCache`. The font size is in physical
  pixels: `App::new` multiplies the configured size by the window's scale
  factor.
- **Tabs and panes are already separate from the window.**
  `SessionRuntime` (`runtime.rs`) is documented as the
  "Window-independent owner for terminal processes and native app
  sessions". It holds the `TerminalManager` and the `ProtocolServer`.
  `ultty-session` and `ultty --attach` already split owner and viewer into
  two processes (`docs/SESSION_ATTACHMENT.md`, `viewer/mod.rs`).
- **But the manager assumes one window.** `TerminalManager`
  (`terminal_manager.rs`) has one `active_tab` and one `pane_area`, and
  `relayout` sizes every tab to that one area. `TerminalId` and `TabId`
  are process-wide counters.
- **Control is per process.** Each process listens on
  `ultty-ctl-<pid>.sock` (`control/paths.rs`). `ulttyctl` picks `--pid`,
  else `ULTTY_PID`, else the newest instance (`bin/ulttyctl.rs`,
  `select_instance`). Shells get `ULTTY_PANE` (`terminal_manager.rs`) and
  `ULTTY_SOCKET` (`app/mod.rs`, `tab_shell_env`). The window commands
  (`focus`, `fullscreen`, `opacity`, `resize`, `screenshot`) act on "the"
  window (`app/control.rs`).
- **Session files collide.** Each process saves its own tabs to the same
  `last.json` on exit (`save_session` in `app/mod.rs`,
  `SessionData::last_session_path` in `session.rs`). With two windows open, the
  one closed last overwrites the other. `SessionData` v1 has `tabs` and
  `active_tab_index` and no notion of a window.
- **Config reload is per process.** `App::reload_config`
  (`app/actions.rs`) applies colours, opacity, fonts and behaviour to this
  window and its manager. `behavior.accessibility` takes effect only at
  start (`keep_start_only`).
- **A panic ends the process.** The release profile sets
  `panic = "abort"` (root `Cargo.toml`), so `catch_unwind` cannot contain
  one in release builds.

### What a window costs today

From `docs/perf/2026-11-07-idle-footprint.md` (Xvfb, lavapipe, release
build; hardware drivers differ):

- an idle window is 120 MB RSS, of which 52 MiB is lavapipe's LLVM, 22 to
  24 MiB GPU memory (the 2048-square atlas alone is 16 MiB), and 13 to
  15 MiB heap that is "almost all" the Vulkan driver. Ultty's own
  allocations were under 7 MiB;
- exec to titled window is 123 ms, of which about 40 ms is winit's event
  loop and display connection, 14 ms the adapter, and 32 to 33 ms device,
  `FontSystem::new` (12 to 13 ms) and pipelines.

A second process pays all of that again. A second window in the same
process would pay for a surface, its swapchain images and per-window
buffers. I have not measured that; slice 1's check does.

### How Ghostty and kitty do it

From their public docs and source, read 2026-11-17. Items marked
unconfirmed were not verified.

- **Ghostty.** One app process with many surfaces. `src/renderer/Thread.zig`
  creates a renderer thread per surface. `src/font/SharedGridSet.zig`
  shares a ref-counted font grid (faces, glyph cache, atlas) between
  surfaces with the same font configuration and size, freed on the last
  release. Whether the GPU context is shared is unconfirmed. On macOS,
  tabs are native AppKit tabs; dragging them between windows is
  AppKit behaviour (unconfirmed in Ghostty's docs). On GTK a tab can be
  dragged to another window (a discussion and a commit say so;
  unconfirmed in the official repo).
- **kitty.** One process with OS windows, tabs and windows. Its man page
  says new launches open a window in the running instance so that it can
  "share a single sprite cache on the GPU". On macOS one shared GL context
  is used (`glfw/nsgl_context.m`); Linux is unconfirmed. `detach_tab`
  moves a tab to a new OS window, and tabs can be dragged between OS
  windows. Remote control addresses windows and tabs with `--match`
  queries on `id`, `title`, `pid`, `cwd` and others.

Both accept that one crash takes down every window.

## Decision (proposed)

One Ultty process owns one `SessionRuntime` and any number of windows.
"New window" opens a window in the same process. A window is a viewer of
some of the runtime's tabs; tabs belong to exactly one window and can
move between windows.

### Ownership

```
AppHandler
  Shared      GPU (instance, adapter, device, queue), font grids, config,
              ConfigWatcher, ControlServer, clipboard
  SessionRuntime
              TerminalManager (tabs, panes, PTYs), ProtocolServer
  windows: map WindowKey -> WindowState
              winit Window, Surface, SurfaceConfiguration,
              per-window renderer state, AccessKit adapter,
              tab bar, input, selection, IME, copy mode, overlays,
              frame pacing, needs_redraw, scale factor, font zoom
```

- **`WindowKey`** is a small process-wide counter, like `TabId`. winit's
  `WindowId` is mapped to it and never leaves the process. Control, the
  session file and logs use `WindowKey`.
- **`TerminalManager` gains windows.** Each `Tab` records its window.
  `active_tab` and `pane_area` move from the manager into a per-window
  record (`tabs: Vec<TabId>`, `active`, `pane_area`), and `relayout`
  sizes each tab to its window's area. Pane and tab ids stay process-wide,
  so `ULTTY_PANE` and `ulttyctl --pane` keep meaning one pane.
- **`App` splits in two.** Process-wide state (runtime, control server,
  config, watcher, clipboard, pending control waits) stays in one place;
  everything listed under "per-window UI state" above moves to
  `WindowState`. `AppHandler::window_event` looks up the window by id.
  `AppEvent::Accessibility` already carries `accesskit_winit::Event`, which
  names its window.

### GPU device, surfaces and caches

- **One instance, adapter, device and queue.** The first window creates
  them as today (`request_gpu`, primary backends, then GL). Later windows
  create only a surface from the same instance and check
  `surface.get_capabilities(&adapter)` is not empty. If it is empty (a
  window on a monitor driven by a different GPU), that window gets its own
  adapter and device as a fallback; it then cannot share textures, so it
  gets its own font grid too.
- **Per-window surface and configuration.** Each window configures its
  surface with its own size and present mode. Wayland frame-callback
  pacing (`app/pacing.rs`, `paced_by_frame_callbacks`) is per surface,
  so `FramePacing` is per window. `Lost`/`Outdated` handling in
  `app/render.rs` stays per window. Device loss affects all windows: the
  shared GPU is rebuilt and every surface reconfigured.
- **Font grids shared by key, as in Ghostty.** Split `TextRenderer` into:
  - a process-wide `FontSystem` (the 703-face database; built once),
  - `FontGrid`s keyed by font configuration and physical size, each with
    its `GlyphAtlas`, `SwashCache` and shaping, run and cluster caches,
    ref-counted and dropped when no window uses them,
  - per-window metrics, row vertex caches (`text_cache.rs`) and GPU
    buffers.

  Two windows on monitors with the same scale and no zoom share one atlas.
  A window with a different scale or zoom gets its own grid, as it would
  today. `cosmic_text::CacheKey` already includes the size, so one atlas
  could hold several sizes; separate grids are simpler to evict and match
  what Ghostty does.
- **Image textures** (`image_cache`, `native_image_cache`) are keyed by
  terminal and image, so they can be shared per device. Keep them per
  window first; share later only if measurement says so.

### Moving tabs between windows

- **Commands first.** "Move tab to new window" and "Move tab to window N"
  in the tab menu and command palette, and `ulttyctl move-tab`. A move
  changes the tab's window, resizes its panes to the new window's pane
  area (the PTYs get `TIOCSWINSZ` as on any resize), and rebuilds both
  windows' tab bars and accessibility trees. PTYs, native sessions and
  scrollback do not move; they never belonged to a window.
- **Drag out of the window.** The tab bar is drawn by Ultty
  (`renderer/tabbar.rs`, CSD), and tabs can already be dragged to reorder
  them (`dragging_tab` in `tabs.rs`), so this is Ultty's own code, not a
  platform tab API. A tab dragged out of the tab bar and released
  outside the window becomes a new window. On X11 and macOS Ultty can
  place the new window at the pointer; on Wayland the compositor places it,
  because clients cannot set or read window positions.
- **Drop onto another window.** winit 0.30 has no cross-window drag and
  drop for in-app data. On X11 and macOS, global pointer and window
  positions would let Ultty hit-test its other windows itself. Wayland
  gives no global coordinates, so dropping a tab onto another window is
  not possible there without a data-device drag that winit does not
  expose. This part is deferred: the move commands cover it. Prototype on
  X11 before promising it anywhere.

### Focus and accessibility

- Each window has its own focused tab and pane. The process records the
  last window that had `WindowEvent::Focused(true)`; control commands with
  no explicit target use it.
- Each window has its own AccessKit adapter (`a11y/adapter.rs` already
  "Owns the AccessKit adapter for one window") and its own tree with
  only its tabs. Live-region state (`a11y/live.rs`) is per window. A tab
  that moves is removed from one tree and added to the other in the same
  update. Every window must be created hidden and shown after its adapter
  exists, as `App::new` does today ("AccessKit must see the window before
  it is first shown").

### Control socket and `ulttyctl`

- Still one socket per process, `ultty-ctl-<pid>.sock`. `--pid` and
  `ULTTY_PID` keep selecting the process.
- New global flag `--window <id>`, and new commands: `windows` (id,
  title, size, tabs, focused), `new-window [cmd]`, `close-window <id>`,
  `move-tab <tab> --window <id>|--new-window`.
- `tabs`, `panes` and `subscribe` events gain a `window` field.
  `window_opened` and `window_closed` events are added.
- Window commands (`focus`, `fullscreen`, `opacity`, `resize`,
  `screenshot`) target, in order: `--window`; the window holding `--pane`
  or `ULTTY_PANE`; the last focused window. Commands run from a shell
  inside Ultty therefore act on that shell's window even after its tab
  moves, without a `ULTTY_WINDOW` variable that would go stale.
- `new-tab` opens in the target window by the same rule.
- This is additive to the control schema: old clients see one more field.
  `describe` lists the new operations.

### Session save and restore

- `SessionData` v2 adds `windows: [{ tabs, active_tab_index, size }]`.
  v1 files load as one window. The writer writes v2 only.
- The process saves all open windows when the last one closes or on
  `ShutdownSignal`. A window closed earlier is gone from the session, as
  its tabs are; that matches closing a tab.
- One process means one writer, which removes today's last-closer-wins
  overwrite between windows. Two separately started processes (for
  example `ultty -e` windows, which do not save) still exist; see
  "Single instance" below.
- Window size is saved in logical pixels. Position is not saved: Wayland
  cannot restore it.

### Single instance

Running `ultty` while an instance is running should open a window in that
instance, as kitty and Ghostty do. Do this after slice 3, behind
`behavior.single_instance` (default off at first): `ultty` connects to the
newest instance's control socket and sends `new-window` with its working
directory and `-e` command, then exits. If no instance answers within a
short timeout, it starts normally. `ultty --new-process` always starts
one.

### Config reload

One `ConfigWatcher` per process. `reload_config` applies colours,
opacity, URL detection and keybindings to every window, and manager
settings once. A font change rebuilds the font grids and relayouts every
window. Per-window font zoom is kept, as `apply_font_config` keeps it now.
`behavior.accessibility` stays start-only for the process.

### Failure isolation

All windows share one fate, as in Ghostty and kitty:

- Release builds abort on panic. Switching to `panic = "unwind"` and
  wrapping per-window handling in `catch_unwind` would leave shared state
  (the manager, the protocol server, GPU objects) half-updated after an
  unwind. That is worse than a clean exit. Rejected.
- What a crash loses is bounded by what exists already: session restore
  brings back tabs, splits and directories (`session.rs`), and users who
  need processes to survive the UI can run them under `ultty-session`
  (`docs/SESSION_ATTACHMENT.md`), whose viewer is a separate process.
- Per-window errors that are not panics stay per window: surface lost or
  outdated, a failed surface creation (the window closes with an error,
  the others keep running), and an AccessKit adapter that fails to
  connect.
- "New window" with `behavior.window_per_process = true` keeps today's
  behaviour for anyone who wants process isolation.

### Memory and the earlier performance work

- Expected: the second window costs a swapchain, per-window buffers and
  vertex caches, plus a font grid only if its scale differs. Everything
  driver-side in the idle-footprint table is paid once. To be measured in
  slice 1.
- **Idle wakeups** (ADR 0007, `docs/perf/2026-11-07-idle-footprint.md`)
  must not grow with windows: `next_update_deadline` becomes the minimum
  over windows and the runtime, and an idle window sets no timers. The
  cursor blinks only while its window has focus (`app/update.rs`), so at
  most one window's blink timer runs.
- **Latency.** All windows render on the one UI thread; separate
  processes rendered in parallel. A window flooding output could delay
  another window's echo. PTY parsing stays on the per-terminal reader
  threads (`event_loop.rs`), so the cost is rendering. Draw only windows
  with `needs_redraw`, and gate on a benchmark: the echo latency of
  window A must stay within 10% of today while window B runs
  `pty_flood`.
- **Memory budgets** (`cargo bench -p ultty --bench memory`) measure the
  terminal model and should not move. `heap_trim` stays process-wide.

## Alternatives considered

- **Keep one process per window.** No work, full isolation, but every
  window pays the GPU driver and font costs, the session file is
  last-closer-wins, the quick terminal needs its own process, and tabs
  cannot move. Kept as an opt-in setting, not the default.
- **Windows as viewers of a shared `ultty-session` host.** Each window a
  separate viewer process attached to one owner (`viewer/mod.rs`). This
  gives isolation and tab sharing, but each viewer still pays the GPU and
  font costs, and moving a tab means a protocol round trip. It is the
  right answer for persistence, not for windows.
- **One thread per window.** winit runs on one thread and creates windows
  only there; wgpu devices are `Send + Sync`, so rendering could move off
  the main thread later. Not needed until the latency gate fails.
- **Native platform tabs** (macOS window tabbing, as Ghostty uses).
  Ultty draws its own tab bar on every platform; native tabs would split
  the behaviour by platform. Not now.

## Slices and rough effort

Estimates for one engineer who knows the codebase, before any prototype.
Each slice also needs `cargo test --workspace`, clippy with
`-D warnings`, `fmt --check` and the GUI smoke.

| Slice | Contents | Effort | Check |
|---|---|---|---|
| 1 | Split `App` into process state and `WindowState`; shared GPU; route events by window id; per-window tab list, active tab and pane area in `TerminalManager`; "New window" opens in-process; last window closing exits | 6–9 days | New `TerminalManager` tests for per-window relayout; GUI smoke opens two windows and types into both; RSS and startup of the second window measured and recorded under `docs/perf/` |
| 2 | Font grids shared by key; one `FontSystem`; per-window zoom | 3–5 days | Atlas count stays 1 for two same-scale windows (`cache_stats`); render bench unchanged |
| 3 | Control: `--window`, `windows`, `new-window`, `close-window`, window field in events; target rules | 3–4 days | `ulttyctl` tests; `describe` lists the operations; existing control tests pass unchanged |
| 4 | Session v2 with windows; restore opens N windows; v1 compatibility | 2–3 days | Round-trip tests for v1 and v2 files |
| 5 | Move tab: commands, menu, palette, `move-tab`; drag-out to a new window | 4–6 days | Move keeps the PTY (pid unchanged) and resizes it; accessibility trees of both windows updated (AccessKit consumer test) |
| 6 | Single instance (`behavior.single_instance`) | 2–3 days | Second `ultty` exits after opening a window in the first |
| 7 | Drop a tab onto another window, X11 and macOS only | 3–5 days, after a prototype | Not planned until slice 5 ships |

Slices 1 to 4 make "New window" in-process and are the minimum to close
gap 9's first half. The quick terminal (ADR 0028) needs slices 1 and 3.

## Consequences

- One crash closes every window. This is the main thing given up.
- A second window should be cheap in memory and start faster. To confirm
  in slice 1.
- `App` (2,142 lines in `app/mod.rs`) and its `impl` blocks in `app/*.rs`
  are split by ownership. Most methods touch either process state or one
  window, so the split should be mechanical, but it touches nearly every
  file in `app/`.
- The control schema grows; existing clients keep working.
- `ultty-session` and the in-process runtime stay two answers to two
  questions: persistence and isolation versus cheap windows.

## Open questions

1. **Last window closed on macOS.** Should the process stay running with
   no windows, as macOS apps usually do, and as the quick terminal would
   need? On Linux, exit unless a quick terminal is configured?
2. **Single instance by default.** Turn `behavior.single_instance` on by
   default once it is stable, as kitty and Ghostty do?
3. **Window-per-process setting.** Is anyone likely to want it, or can
   the old behaviour be dropped with slice 1?
4. **Drop onto another window.** Worth slice 7 at all, given that it
   cannot work on Wayland?
5. **Saving closed windows.** Should a window closed before the last one
   be kept in the session (as browsers keep "recently closed")?
