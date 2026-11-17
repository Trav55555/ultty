# Command palette: headless GUI smoke

Run 2026-11-17 on branch `command-palette`, debug build, on a private Xvfb
(no Wayland, private `XDG_RUNTIME_DIR` and config dir, `ulttyctl --pid`).
`palette-shot.sh` drives the window with `xdotool` and captures with
`ulttyctl screenshot`:

```bash
scripts=docs/releases/evidence/2026-11-17/command-palette
cargo build -p ultty --bins
$scripts/palette-shot.sh /tmp/palette-dark 1280x800
$scripts/palette-shot.sh /tmp/palette-light 1280x800 light
RESIZE="480 300" $scripts/palette-shot.sh /tmp/palette-small 1280x800
```

| File | Shows |
|---|---|
| `dark-open.png` | Ctrl+Shift+P, empty query: actions in table order with their keys, the count, the placeholder |
| `dark-filtered.png` | `sp` then Down: fuzzy matches ranked, matched letters colored and underlined, second row selected |
| `dark-after-split-right.png` | `split right` + Enter: the pane split, the palette closed |
| `light-filtered.png` | the same in a light color scheme |
| `small-filtered.png` | a 480x300 window: the panel fits and the list scrolls |
| `small-split-no-match.png` | `zzq` over a split: "No matching commands" |
| `small-pane-text.txt` | `ulttyctl text` afterwards: none of the typed queries (`sp`, `split right`, `zzq`) reached the shell |
