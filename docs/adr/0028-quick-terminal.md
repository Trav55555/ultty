# ADR 0028: Quick Terminal

## Status

**In progress** — Slices 1, 3, and 4 landed (2026-11-24):
- Slice 1: Quick window role (`WindowRole::Quick`, app ID / `WM_CLASS`
  `ultty-quick`), `[quick_terminal]` config (`position`, `size`, `autohide`,
  `hotkey`), `ultty --quick` (handoff to running instance or cold-start
  quick-only process), `ulttyctl toggle-quick` / `show-quick` / `hide-quick`
  with `XDG_ACTIVATION_TOKEN` / `DESKTOP_STARTUP_ID` forwarding, X11
  position/size/always-on-top/hide-show, Wayland drop-and-recreate while
  preserving tabs and PTYs, focus-loss autohide, session exclusion, and
  compositor setup snippets in [docs/quick-terminal.md](../quick-terminal.md).
- Slice 3: X11 global hotkey via `x11rb` (`XGrabKey` on the root window across
  CapsLock/NumLock modifier variants, live re-registration on config reload,
  clear `AlreadyGrabbed` conflict reporting, zero-polling wakeup loop),
  verified under Xvfb with `xdotool key`.
- Slice 4: Wayland `org.freedesktop.portal.GlobalShortcuts` portal integration
  via `zbus` (non-blocking background session, `[keybindings]` to XDG shortcuts
  spec `preferred_trigger` translation, `Activated` `activation_token`
  forwarding, clean `Session::Close` on drop/reload, and graceful fallback
  message on compositors without the portal).

Slices 2 and 5 are not yet implemented. Original proposal and decisions are
from 2026-11-17 (code references in "What Ultty has today" were to `main` at
252c082; winit references are to 0.30.13).

## Context

### The gap

`docs/feature-parity.md` gap 9: Ultty has no quick terminal. Ghostty has
one on macOS and on Wayland. A quick terminal is a terminal window that a
global hotkey shows and hides, usually sliding down from the top edge of
the screen, keeping its tabs while hidden.

### What Ultty has today

- No quick terminal, and no config for one (`config.rs`).
- One window per process (ADR 0027). The window is created undecorated
  and transparent, with Wayland app id and X11 `WM_CLASS` `ultty`
  (`App::new`, `app/mod.rs`, `WindowAttributesExtWayland::with_name`).
- `ulttyctl focus` raises the window (`app/control.rs`). Nothing hides it.
- Ultty does not handle `XDG_ACTIVATION_TOKEN` or call winit's
  activation-token API anywhere in `crates/ultty/src`.
- `smithay-client-toolkit` 0.19.2, `wayland-protocols-wlr` 0.3.12,
  `x11rb` 0.13.2 and `zbus` 5.19.0 are already in `Cargo.lock`, pulled in
  by winit and AccessKit. Ultty does not use them directly.

### What winit 0.30 can and cannot do

From the winit 0.30.13 docs and source:

- `Window::set_visible`: "Android / Wayland / Web: Unsupported."
- `Window::focus_window`: "iOS / Android / Wayland / Orbital: Unsupported."
- `Window::set_outer_position` and `outer_position`: unsupported on
  Wayland.
- X11 has `WindowAttributesExtX11::with_override_redirect` and
  `with_x11_window_type` (`WindowType::Dock`, `Utility`, `Dropdownmenu`
  and others) (`src/platform/x11.rs`).
- `WindowLevel::AlwaysOnTop` exists; the docs say it is unsupported on
  Wayland.
- macOS has `with_activation_policy` (for example no Dock icon),
  `with_titlebar_hidden` and similar (`src/platform/macos.rs`), but no
  `NSPanel`.
- No wlr-layer-shell. winit issue #2582 asks for it; a maintainer
  deferred it to 0.31, and the work is in an open PR (#4044) as of the
  latest comment I found. 0.31 is in beta; I could not confirm it
  includes layer shell, so assume not.
- Activation tokens exist on Wayland and X11:
  `WindowAttributesExtStartupNotify::with_activation_token` and
  `request_activation_token` (`src/platform/startup_notify.rs`, which
  reads `XDG_ACTIVATION_TOKEN`).

### How others do it

Read 2026-11-17 from public docs; unconfirmed items marked.

- **Ghostty.** `keybind = global:cmd+backquote=toggle_quick_terminal`.
  macOS since 1.0, Linux since 1.2.0. On Linux it needs Wayland with
  `wlr-layer-shell` (through gtk4-layer-shell), so it does not work on
  GNOME (mutter has no layer shell) or on X11. One quick terminal per
  instance; not restored on restart; tabs on Linux but not macOS.
  `global:` keybinds need Accessibility permission on macOS, and the XDG
  `GlobalShortcuts` portal on Linux (KDE 5.27+, GNOME 48+ "should work",
  Hyprland with setup, not sway or other wlroots compositors).
- **kitty.** The `quick-access-terminal` kitten (0.42) is built on the
  `panel` kitten. Running it again hides the window. It has no built-in
  global hotkey: on Linux the user binds the command in the compositor or
  desktop; on macOS the user assigns a Services shortcut. Panels use
  layer shell on Wayland (good on Hyprland, labwc, niri, river; partial
  on sway and KDE; not GNOME), `_NET_WM` hints on X11 (depends on the
  window manager), and a window level on macOS.

### Global hotkeys by platform

- **Wayland.** A client cannot grab keys. The choices are the
  `org.freedesktop.portal.GlobalShortcuts` portal (KDE, Hyprland, GNOME 48+
  per Ghostty's docs; not the wlr portal, so not sway) or a compositor
  keybinding that runs a command. Every compositor can run a command.
- **X11.** `XGrabKey` on the root window works under any window manager,
  unless another client holds the same combination. The `global-hotkey`
  crate does this on X11, and also macOS and Windows, but not Wayland.
- **macOS.** Carbon `RegisterEventHotKey` (used by `global-hotkey`) is
  reported not to need Accessibility permission; event taps do. I found no
  Apple document that says so; treat it as unconfirmed until tested.
  Ghostty's docs say its `global:` binds need Accessibility permission.

## Decision (proposed)

The quick terminal is a window of the multi-window process (ADR 0027)
with a `Quick` role. Ship the toggle command first and leave the hotkey
to the desktop, as kitty does. Add platform polish one slice at a time.

### The quick window

- At most one per process. It is a `WindowState` like any other: its own
  tabs, focus, accessibility adapter and pane area. Tabs can move between
  it and normal windows (ADR 0027 slice 5).
- Hiding keeps its tabs and processes. They belong to the
  `SessionRuntime`, not the window (ADR 0027), so on platforms where
  winit cannot hide a window, the quick window's winit `Window` and
  surface can be dropped on hide and created again on show without losing
  anything but GPU buffers.
- It keeps the process alive while it exists, hidden or not. Closing the
  last normal window does not exit if a quick window exists.
- Not saved in the session, as in Ghostty. Its tabs end with the
  process.
- Excluded from "last focused window" defaults for `ulttyctl` while
  hidden.

### Commands

- `ulttyctl toggle-quick`, `show-quick`, `hide-quick`. The control
  request carries the caller's `XDG_ACTIVATION_TOKEN`, if set, so the
  shown window can take focus on compositors that enforce activation.
- `ultty --quick`: toggle the quick terminal of the running instance, or
  start an instance with only the quick terminal shown. This is the one
  command to bind in a compositor. It uses the single-instance path of
  ADR 0027 (connect to the newest control socket, send the request, exit).
- `quick_terminal.autohide` (default true): hide when the quick window
  loses focus.

### Config

```toml
[quick_terminal]
position = "top"      # top, bottom, left, right, center
size = "40%"          # of the monitor along the position axis
autohide = true
# hotkey = "ctrl+grave"   # slice 3 onwards; ignored where unsupported
```

`hotkey` uses the same key syntax as `[keybindings]` and is parsed by the
keybinding parser in `config.rs`.

Where a platform cannot honour position (Wayland without layer shell), it
is a hint and the docs say so.

### Wayland

Two ways to draw the window:

1. **An xdg toplevel with app id `ultty-quick`** (slice 1). winit can
   create it now. Hide drops the `Window`; show creates it again with the
   activation token. Size is set with `with_inner_size`; position, layer
   and "on all workspaces" come from compositor window rules keyed on the
   app id. The docs give rules for sway, Hyprland, niri and KDE. GNOME has
   no window rules without an extension, so on GNOME it is an ordinary
   window that appears and disappears. This works everywhere, honestly
   labelled as "positioned by your compositor".
2. **A wlr-layer-shell surface** (slice 2). Anchored to an edge, above
   other windows, the way Ghostty and kitty do it. winit cannot create
   one. The options:
   - wait for winit layer shell (#2582, PR #4044) and adopt it when it is
     released. No extra code, unknown date;
   - use `smithay-client-toolkit`'s `shell::wlr_layer` on winit's Wayland
     connection (its `wl_display` is available from the display handle,
     and `wayland-backend` can wrap a foreign display), and create the
     wgpu surface from the raw `wl_display` and `wl_surface` handles.
     wgpu takes raw handles, so the shared device of ADR 0027 can draw to
     it; I have not tried it. The cost is that winit delivers no events
     for a surface it did not create: keyboard (with xkbcommon), pointer,
     IME (`text-input-v3`), scale and frame callbacks would all need
     Ultty's own Wayland code, and `accesskit_winit` would not apply
     (AccessKit's Unix adapter would have to be used directly).

   Recommendation: do slice 1, then prototype the SCTK path for keyboard
   and frame callbacks only, and decide between it and waiting on winit
   from what the prototype costs. GNOME has no layer shell either way.

Global hotkey on Wayland: the compositor binding to `ultty --quick` is
the supported path. The `GlobalShortcuts` portal (through `zbus`, already
a dependency, or `ashpd`) is slice 4, for KDE, GNOME 48+ and Hyprland
users who want the binding in Ultty's config.

### X11

- `set_visible` works on X11, so hide and show keep the window.
- The window is undecorated already; set `WindowLevel::AlwaysOnTop`, and
  position and size it on the monitor under the pointer with
  `set_outer_position` and `request_inner_size`.
- Window type: a normal (or `Utility`) window, not override-redirect.
  Override-redirect windows bypass the window manager, so it does not
  give them keyboard focus and Ultty would have to call `XSetInputFocus`
  itself and handle focus loss. `Dock` windows are often not given
  keyboard focus either (kitty reports dock behaviour varies by window
  manager). Try `Utility` and `Normal` with always-on-top first; keep
  override-redirect as a fallback behind a setting if a window manager
  misbehaves.
- Global hotkey (slice 3): `XGrabKey` through `x11rb` or the
  `global-hotkey` crate, from `quick_terminal.hotkey`. Report a clear
  error when the grab fails because another client holds it.

### macOS

- winit creates an `NSWindow`, not an `NSPanel`. Through `objc2` on the
  raw window handle: set the window level (winit's `AlwaysOnTop` maps to
  a floating level), and collection behaviour so it can show on every
  Space and over full-screen apps. Show activates the app; hide returns
  focus to the previous app. A true non-activating `NSPanel` would need a
  window outside winit; not planned unless the window approach fails.
- Global hotkey: Carbon `RegisterEventHotKey` through `global-hotkey`,
  which needs the main-thread event loop winit already runs. Test whether
  it really works without Accessibility permission on a signed build.
- No compositor to bind a command in, so on macOS the hotkey is the
  feature. `ultty --quick` still works from tools like Shortcuts or
  skhd.
- This slice needs a Mac to develop on and macOS CI (ADR 0029) to keep.

## What to ship first

Slice 1: `ultty --quick` and `ulttyctl toggle-quick` on Wayland and X11,
with the hotkey left to the desktop, and docs with compositor snippets.
It needs only winit APIs, works on every Linux desktop including GNOME,
and is what kitty ships as its whole Linux story.

### Decisions (2026-11-17)

1. **Recreate or keep on Wayland.** Recreate: hide drops the winit
   window and show creates it again, as designed. Slice 1 measures the
   show time against 50 ms; minimising is not a fallback, because winit
   cannot un-minimise on Wayland.
2. **Saving the quick terminal's tabs.** No, as in Ghostty. Its tabs end
   with the process. Decided by default; revisit if needed.
3. **One per process or per monitor.** One per process, as in Ghostty and
   kitty. It opens on the monitor under the pointer where the platform
   allows placement.
4. **Hotkey syntax.** A separate `quick_terminal.hotkey` string, parsed
   with the keybinding parser of `config.rs`, so there is one key syntax.
   Decided by default; revisit if needed.

## Alternatives considered

- **A separate quick-terminal process.** Possible today without ADR 0027:
  `ultty --quick` starts a process that owns one window and toggles via
  its control socket. It duplicates GPU and font costs, and its tabs
  cannot move to normal windows. Acceptable as a stopgap only if ADR 0027
  is not done; then the code would be thrown away.
- **Layer shell first, like Ghostty.** Best result on wlroots compositors
  and KDE, none on GNOME or X11, and the most code or the longest wait.
  Second, not first.
- **Built-in hotkey first.** Not possible on Wayland without the portal,
  and the portal is missing on sway and other wlroots compositors.

## Slices and rough effort

Estimates for one engineer who knows the codebase, after ADR 0027 slices
1 and 3. Each slice also needs `cargo test --workspace`, clippy with
`-D warnings`, `fmt --check` and the GUI smoke.

| Slice | Contents | Effort | Check |
|---|---|---|---|
| 1 (**Done**, 2026-11-24) | Quick role; `ultty --quick`; `toggle-quick`, `show-quick`, `hide-quick`; X11 position, size, always-on-top, hide/show; Wayland toplevel `ultty-quick` dropped and recreated, activation token forwarded; autohide; docs with sway, Hyprland, niri, KDE, GNOME and X11 binding snippets | 4–6 days | `config::tests::test_quick_terminal_config_defaults_parsing_and_geometry`, `control::single_instance::tests::quick_handoff_forwards_activation_token_and_reports_visibility`, and `scripts/e2e/quick-terminal.sh` (toggles twice under Xvfb, shell PID unchanged, X11 mapped state flips, hidden quick excluded from default targets/session save, keeps process alive when last normal window closes) |
| 2 | Wayland layer shell: prototype SCTK path (keyboard, frame callbacks), then either finish it or adopt winit's | 2–3 days prototype; 8–12 days to finish | Prototype report with a go/no-go |
| 3 (**Done**, 2026-11-24) | X11 global hotkey from `quick_terminal.hotkey` (keybinding syntax) | 2–3 days | `platform::x11_hotkey::tests::*` and `scripts/e2e/quick-terminal.sh` under Xvfb with `xdotool key ctrl+grave` (toggles on/off/on keeping PID, plus second-instance grab conflict warning) |
| 4 (**Done**, 2026-11-24) | Wayland `GlobalShortcuts` portal | 2–4 days | `platform::wayland_portal_hotkey::tests::*` (XDG trigger translation, request handle sanitization, activation token extraction, unsupported-portal classification); graceful message on desktops without the portal |
| 5 | macOS: level, Spaces, activation, Carbon hotkey | 4–6 days on a Mac | macOS CI build (ADR 0029); manual check recorded |
| 6 | Slide animation | not planned | |

## Consequences

- The quick terminal is cheap once windows are multi-window: most of it
  is window attributes and a toggle command.
- On Wayland the first version depends on the user's compositor rules for
  placement. That is a weaker result than Ghostty's on wlroots and KDE,
  and a better one on GNOME, where Ghostty has none.
- Until slice 2, "quick terminal" on Wayland means "a terminal window that
  toggles", and the docs must say that.

## Open questions

1. **Activation tokens.** Do sway, Hyprland and GNOME set
   `XDG_ACTIVATION_TOKEN` for commands run from a keybinding? If not, the
   window may appear without focus on compositors that enforce
   activation. Needs testing per compositor.
