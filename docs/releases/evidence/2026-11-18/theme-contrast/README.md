# Selection colours and missing-font fallback

Headless runs of a debug build: private Xvfb, `HOME`, `XDG_*` and runtime
dir, `ulttyctl --pid`, selection made with `xdotool`. PNGs are quantized to
64 colours.

The `*-before` images are the 2026-11-17 smoke pass's (`smoke/16`, `15`, `13`
and `00`, whose screen text differs from the `*-after` ones); the `*-after`
images show the same cases on this branch.

- `nord-*`: selected text was `#d8dee9` on `#eceff4`; now the theme's own
  selection foreground.
- `solarized-light-*`: the selection was barely distinguishable from the
  background; the importer moves it until it shows.
- `default-*`: the default theme's selection is clearly visible.
- `missing-font-*`: no `DroidSansMono Nerd Font` installed; the grid is
  monospace (one warning is logged).
- `search-status-after.png`: `ulttyctl screenshot` now includes the copy-mode
  status line (`-- NORMAL -- quick [1/1]`).
