# Headless visual smoke pass, 2026-11-17

A pass over features merged in the days before, at `origin/main` 252c082,
debug build. Each run used a private Xvfb, `env -i` with a throwaway `HOME`,
private `XDG_RUNTIME_DIR` and `XDG_CONFIG_HOME`, and `ulttyctl --pid`. Keys
went in with `xdotool`. Screenshots come from `ulttyctl screenshot` except
those named `*-xgrab`, which are `ffmpeg -f x11grab` frames of the real
window (see "Screenshot gap" below). The PNGs are quantized to 256 colours
to keep them small.

The throwaway `HOME` has no `~/.local/share/fonts`, so the default family
(DroidSansMono Nerd Font) is missing there; after the first screenshot the
user's font directory was linked in so the rest look like a normal setup.

## Results

| # | Check | Result | Screenshots |
|---|---|---|---|
| 1 | Palette over a native app (counter-app) | pass | `01`–`05` |
| 1 | Keys typed into the palette (`+++q`) do not reach the app; `+` after Escape does | pass | `03`–`05` |
| 1 | Split right from the palette; palette over a split | pass | `06`, `07` |
| 2 | `ultty --list-themes` lists 94 themes | pass | – |
| 2 | light, default, Dracula, Solarized Light, Nord, Rosé Pine, Catppuccin Latte, Gruvbox Dark, switched live by editing the config | pass | `10`, `13`–`19` |
| 2 | Cursor, tab bar and palette on a light theme | pass | `10`, `12`, `21` |
| 2 | Selection colours | **fail** (Nord and others) | `11`, `13`–`19` |
| 2 | Synthetic bold on light themes | **fail, fixed** | `10`, `15`, `18` |
| 2 | `themes/dracula.toml` in the private config dir overrides the bundled one | pass | `20` |
| 3 | Search highlights, wrapped match, `[3/3]` count, `n` | pass | `41`, `42`, `44` |
| 3 | Search prompt/status readable | **fail, fixed** | `43`, `44` |
| 3 | Regex mode (Ctrl+R) | **fail, fixed** | `46` |
| 4 | bash: integration injected, prompt jumps, Ctrl+Shift+O | pass | `51`–`54` |
| 4 | zsh: integration injected (`ULTTY_SHELL_INTEGRATION=auto`), jumps, Ctrl+Shift+O | pass | `56`, `57` |
| 4 | New tab opens in the shell's directory (bash and zsh) | pass | `55` |
| 5 | Font size and family changed in the config apply live | pass | `30`–`32` |
| 5 | Bold and italic with a family that has the faces (Liberation Mono) | pass | `32` |
| 5 | OpenType features | not checked: no installed font with visible alternates | – |
| 5 | Default family not installed | **fail** | `00` |
| 6 | Image scrolls into scrollback with its text | pass | `60`–`62` |
| 6 | `clear` removes it | pass | `64`, `65` |
| 6 | Text over an image, EL 2, EL 0 cut it | pass | `66` |
| 6 | Tall image (1200 px) scrolls the screen | pass | `67`, `68` |
| 6 | less and nvim (alternate screen) hide main-screen images | pass | `69`–`71` |
| 6 | Image in a split pane, clipped to the pane | pass | `72`, `73` |

## Problems

**Regex search never matched (fixed).** The app turns a bare Shift or Ctrl
press into `Key::Char('\0')`, and the search prompt appended it to the query.
Ctrl+R puts a NUL before the pattern, so every regex search found nothing
(`46`), and so did any literal query typed with Shift (capitals, `+`, `?`).
The prompt now ignores control characters.

**Search status line half-covered (fixed).** The copy-mode indicator's
background was sized at 0.6 cells per character, so the query and the
`[n/m]` count were drawn over the shell's last row (`43`, `44`). It is now
sized by the text's column width.

**Synthetic bold was paler on light themes (fixed).** Without a bold face,
bold text is drawn in a brightened colour. On a light background that moves
it towards the background: "bold" in `10`, `15` and `18` is fainter than plain
text. Bold is now moved away from the background: darker on a light theme.

**Selection unreadable or invisible on many bundled themes (not fixed).**
Nord's selection is `#eceff4` under `#d8dee9` text, so selected text
disappears (`16`). Its upstream scheme sets `selection-foreground`, which the
importer drops and Ultty does not support. A contrast check over the 94
themes, with the selection drawn as Ultty's translucent overlay, finds most
with selected text under 2.5:1 and about 14 (Solarized Light and Dark,
Everforest Light, Hybrid, Everblush, Nord Light, ...) whose selection is
barely distinguishable from the background. The default theme's selection is
also faint (`13`). Needs selection-foreground support or importer changes.

**Default font missing gives a broken grid (not fixed).** With no
`DroidSansMono Nerd Font` installed, as on a fresh machine, cosmic-text falls
back to a proportional family and text is spread out (`00`).
`family = ""` (system monospace) renders well in the same setup. Falling back
to the system monospace when the configured family is missing would fix it;
it changes several font-selection tests, so it is left for a separate change.

**Screenshot gap (not fixed).** `ulttyctl screenshot` does not draw the
copy-mode indicator (search prompt, `[n/m]`), so `40`–`42` show highlights
but no status line; the `-xgrab` frames show what the window draws.

## After the fixes

Rebuilt from this branch and rerun on Catppuccin Latte:

- `80-fixed-regex-search-xgrab.png`: Ctrl+R, `ne+dle`, Enter finds 63
  matches (`nedle`, `needle`, `neeedle`), the wrapped one included, and the
  status line sits on a background as wide as its text.
- `81-fixed-bold-light-theme.png`: synthetic bold is darker than plain text.

`cargo test -p ultty --lib`: 1314 passed.

## Notes

- After a sixel, text continues on the image's last row, as in xterm and foot
  (`60`); documented in `Terminal::add_image`.
- Splitting keeps focus in the original pane, from the palette and from the
  key alike.
- The bundled `light` theme's colour 3/7 and bright 3/7 are hard to read on
  its white background (`10`).
