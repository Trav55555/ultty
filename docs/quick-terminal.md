# Quick Terminal

Ultty includes a drop-down / toggleable quick terminal window
([ADR 0028](adr/0028-quick-terminal.md)). At most one quick terminal exists per
Ultty process; hiding it preserves all of its tabs and running processes, and
you can move tabs between the quick terminal and normal windows.

## Usage

Bind a desktop or compositor shortcut to:

```sh
ultty --quick
```

- If an Ultty process is already running, `ultty --quick` toggles that
  instance's quick terminal (forwarding `XDG_ACTIVATION_TOKEN` /
  `DESKTOP_STARTUP_ID` so the compositor can focus it) and exits immediately.
- If no Ultty process is running, `ultty --quick` starts one with only the
  quick terminal shown.
- You can also control it explicitly with `ulttyctl`:
  - `ulttyctl toggle-quick`
  - `ulttyctl show-quick`
  - `ulttyctl hide-quick`

Quick terminal tabs are not saved to `sessions/last.json` or named workspaces,
and while hidden the quick terminal is excluded from `ulttyctl`'s default
focused-window target. Closing your last normal window keeps the process alive
while a quick terminal exists (even when hidden).

## Configuration

```toml
[quick_terminal]
position = "top"     # top, bottom, left, right, center
size = "40%"         # percentage of the monitor axis (10%-100%) or pixels (e.g. "420px")
autohide = true      # hide when the quick terminal loses focus
hotkey = "ctrl+grave" # built-in global hotkey on X11 (uses [keybindings] syntax)
```

## Platform behavior & compositor snippets

- **X11:** Ultty positions and sizes the quick window on the active monitor
  according to `position` and `size`, sets always-on-top, hides/shows the same
  window (`WM_CLASS` `ultty-quick`), and registers `quick_terminal.hotkey`
  directly via `XGrabKey` (updating live on config reload and logging a clear
  warning if another client already holds the grab). You can still bind
  `ultty --quick` in your WM if you prefer.
- **Wayland (Slice 1):** Ultty creates an `xdg_toplevel` with app ID
  `ultty-quick` (dropped on hide, recreated on show while keeping tabs/PTYs
  alive). Because Wayland clients cannot self-position ordinary toplevels
  without `wlr-layer-shell` (planned for ADR 0028 Slice 2), placement and
  floating/sticky behavior come from your compositor's window rules below. On
  GNOME (which has no layer shell or built-in window rules), the quick terminal
  behaves as an ordinary window that toggles open and closed.

### sway

`~/.config/sway/config`:

```sway
bindsym $mod+grave exec ultty --quick
for_window [app_id="ultty-quick"] floating enable, sticky enable, border none, move position 0 0, resize set 100ppt 40ppt
```

### Hyprland

`~/.config/hypr/hyprland.conf`:

```ini
bind = SUPER, grave, exec, ultty --quick
windowrulev2 = float, class:^(ultty-quick)$
windowrulev2 = pin, class:^(ultty-quick)$
windowrulev2 = move 0 0, class:^(ultty-quick)$
windowrulev2 = size 100% 40%, class:^(ultty-quick)$
```

### niri

`~/.config/niri/config.kdl`:

```kdl
binds {
    Mod+Grave { spawn "ultty" "--quick"; }
}

window-rule {
    match app-id=r#"^ultty-quick$"#
    open-floating true
    default-floating-position x=0 y=0 relative-to="top"
}
```

### KDE Plasma

1. **Shortcut:** System Settings → Keyboard → Shortcuts → Add New → Command or
   Script → `ultty --quick` (for example `Meta+\``).
2. **Window rule:** System Settings → Window Management → Window Rules → Add
   New:
   - Window class (application): Exact Match `ultty-quick`
   - Keep above other windows: Force → Yes
   - Position: Apply Initially → `0,0`
   - Skip taskbar / Skip pager: Force → Yes

### GNOME

Settings → Keyboard → View and Customize Shortcuts → Custom Shortcuts → Add:
- **Name:** Ultty Quick Terminal
- **Command:** `ultty --quick`
- **Shortcut:** `Super+\`` (or your preference)

### X11 (i3 / sxhkd / xbindkeys)

**i3** (`~/.config/i3/config`):

```i3
bindsym $mod+grave exec --no-startup-id ultty --quick
for_window [class="ultty-quick"] floating enable, sticky enable, border none
```

**sxhkd** (`~/.config/sxhkd/sxhkdrc`):

```text
super + grave
    ultty --quick
```
