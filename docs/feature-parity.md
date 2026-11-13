# Feature parity: Ultty, Ghostty and Alacritty

- Date: 2026-11-11
- Ultty: `main` at `c676314b8c14f3962591c3c232315770d80fe1a9`
- Ghostty: 1.4.0 (released 2026-10-27)
- Alacritty: 0.16.1 (released October 2025). Alacritty 0.17.0 (April 2026)
  is its latest release. This page compares 0.16 because that is the version
  in the [T4 three-way benchmark](perf/2026-11-11-t4-three-way.md). Rows
  that 0.17 changes are noted.

This page records what Ultty lacks or does less well than Ghostty and
Alacritty. Ultty's statuses come from its code and tests. Docs and comments
alone were not taken as proof. Ghostty and Alacritty statuses come from their
official docs, release notes and changelogs (sources at the end). A cell
marked "unverified" means the official pages read did not settle it.

The `scorecard`, `conformance`, `command_marks` and `selection` tests in
`crates/ultty-vt` passed on this SHA (92 tests).

## Biggest gaps

Ranked by how much a typical user would notice:

1. **New tabs and splits open in the focused pane's directory.** *Fixed.*
   Ultty used to spawn them in the directory it was started in. They now use
   the pane's last local OSC 7 directory, else its foreground process's
   directory on Linux (`behavior.inherit_working_directory`, on by default).
2. **Font configuration is thin.** *Mostly fixed.* Ultty now has separate
   bold, italic and bold italic families (`font.bold_family` and the like), an
   OpenType feature list (`font.features`), a codepoint-to-font map
   (`[[font.codepoint_map]]`) and font changes on config reload. It still has
   no variable-font axes and no cell offsets.
3. **No Kitty graphics protocol.** Ghostty supports it (1.4 added
   animation). Programs that only speak Kitty's protocol, such as many
   image viewers and some Neovim plugins, show no images. Ultty supports
   sixel instead, which Ghostty does not. A sliced plan is proposed in
   [ADR 0026](adr/0026-kitty-graphics.md); its slice 0 (sixel scroll,
   clear, split-pane and cache fixes) is done.
4. **Few command-line options.** `ultty` accepts only `--help`, `--version`
   and (added after this audit) `-e command`. There is no
   `--working-directory`, `--config` or per-key override.
   Alacritty has `-e`, `--working-directory` and `-o key=value`. Ghostty has
   `-e` and `--key=value` for every config key.
5. **Scrollback search is plain substring only.** *Fixed.* Search joins
   soft-wrapped rows, uses smart case, has an optional regex mode
   (`[search] regex`, or Ctrl+R in the prompt), highlights every match, counts
   them, and scans in 2 ms slices. It is still limited to copy mode: there is no
   Ghostty-style search bar, and matching is per scrollback snapshot (see
   `docs/configuration.md`).
6. **No jump-to-prompt.** *Fixed.* Ctrl+Shift+PageUp and PageDown scroll to
   the previous and next prompt, and Ctrl+Shift+O selects the last command's
   output when it fits on screen. Longer output cannot be selected, because
   a selection is made of view rows.
7. **Shell integration is manual.** `dist/shell-integration` has bash and zsh
   scripts that the user must source. Ghostty injects its integration into
   bash, zsh, fish, elvish and nushell automatically.
8. **Only six bundled themes.** Ghostty ships hundreds of themes (1.4 alone
   added 356) and can switch between a light and a dark theme
   automatically.
9. **One window per process, and no quick terminal.** "New window" starts a
   second Ultty process. Ghostty has real multiple windows, a quick terminal
   on macOS and Wayland, and a command palette.
10. **Linux x86_64 only in practice.** Release builds and CI cover Linux
    x86_64 only. macOS code exists but is not built or tested by CI. There
    is no Windows or BSD support. Alacritty runs on Linux, BSD, macOS and
    Windows.

Smaller gaps: no SGR-pixel mouse (1016), no background blur or image, no
audible bell, cursor and foreground/background colours cannot be set with OSC
10/11/12, OSC 52 reads are refused (Ghostty can ask), and there are no
key sequences or key tables.

## Where Ultty is ahead

- **Native protocol.** Apps send layout constraints and content over a
  socket, or in-band over ssh, and the terminal lays them out and renders
  them. Neither Ghostty nor Alacritty has anything like it.
- **Automation.** `ulttyctl` reads screen text, the selection and panes;
  sends text, keys and raw bytes; creates tabs and splits; waits for
  patterns; subscribes to command, bell and progress events; takes
  screenshots; and runs as an MCP server (`ulttyctl mcp`). Alacritty's
  `alacritty msg` can open windows and change config. Ghostty can automate
  with AppleScript on macOS and with `+new-window` and similar actions on
  Linux.
- **Screen reader on Linux.** AccessKit exposes the window over AT-SPI, with
  each pane as a `Terminal` node holding its row text and caret
  (`crates/ultty/src/a11y/tree.rs`). Alacritty has no screen reader support.
  Ghostty's support is macOS only.
- **Sixel.** Neither Ghostty nor Alacritty 0.16 supports it.
- **Copy mode.** Vim-style copy mode with character, line and block
  selection. Alacritty has a vi mode too. Ghostty has none.
- **Speed.** See the [T4 three-way comparison](perf/2026-11-11-t4-three-way.md).
- **Automatic config reload.** Ultty watches its config file. Ghostty, as
  far as its docs show, reloads only when asked (`reload_config`).

## Surprises

- **OSC 7 claim (fixed).** The compat scorecard said OSC 7 is used for
  "opening a new tab where the shell is" while the code did not do it. New
  tabs and splits now open in the focused pane's directory.
- **Accessibility docs undersold the code (fixed).** `docs/configuration.md`
  said "for now the window is all it exposes". The AccessKit tree includes
  terminal rows, the caret and native-app regions (`a11y/tree.rs`,
  `a11y/scene.rs`), and the page now says so.
- **Session restore was undocumented (fixed).** `behavior.restore_session`
  (default `false`) restores tabs, split layout and working directories, but
  not scrollback. `docs/configuration.md` now describes it.
- **Wrong shortcut in a doc comment (fixed).** The copy-mode module comment
  (`crates/ultty/src/vim/mod.rs`) said Ctrl+Shift+V. The actual default,
  `keybindings.vim_mode`, is Ctrl+Shift+Space; Ctrl+Shift+V is paste.

## Full table

Status key: **yes**, **partial** (says what is missing), **no**, **unknown**.

### VT and protocol

| Feature | Ultty | Ghostty 1.4 | Alacritty 0.16 | Ultty evidence |
|---|---|---|---|---|
| xterm compatibility breadth | partial: VT100/VT220 core, DEC modes, queries; no DECSLRM, DECSCA, rectangular ops, VT52 | yes; esctest in CI, 1.4 added DECRQCRA, DECBKM, mode 2033 | partial; `CSI h/l` only modes 4 and 20, `CSI t` only 22/23 | `ultty-vt/tests/conformance.rs` (59 tests); README "Not implemented" list |
| Kitty keyboard protocol | yes: flags 1, 2, 4, 8, 16; push, pop, query; per-screen stacks | yes | partial; implemented, with open mismatch reports (issue #8792) | `ultty/src/keys.rs` tests `kitty_*`; `conformance.rs` `kitty_keyboard_*` |
| modifyOtherKeys | yes: levels 1 and 2 | yes; level 2 matches xterm since 1.4 | no entry in escape docs | `keys.rs` `modify_other_keys_level_*` |
| Bracketed paste | yes: ESC and C1 stripped from paste | yes | yes | `scorecard_bracketed_paste` |
| Focus events (1004) | yes | yes | yes | `scorecard_focus_reporting` |
| Mouse: X10, 1000/1002/1003, SGR 1006, 1005, 1015, 1007 | yes | yes | partial; 1003 and 1015 not listed | `term_handler.rs` mode table; `scorecard_sgr_mouse`; `utf8_mouse_encodes_large_coordinates_as_utf8` |
| Mouse: SGR-pixel (1016) | no | unverified in docs | no | no handler for 1016 in `ultty-vt` |
| Synchronized output (2026) | yes, with DECRQM | yes | yes | `scorecard_synchronized_output` |
| OSC 8 hyperlinks | yes; Ctrl+click opens http, https, mailto, ftp only | yes; `link-osc8` config | yes | `scorecard_hyperlinks`; `ultty/src/url.rs` scheme allow-list |
| OSC 52 clipboard | partial: writes (can be denied); reads always refused | yes; read `ask` by default, write `allow` | partial; copy only by default, paste opt-in | `scorecard_clipboard_write`; `ansi/osc.rs` refuses `?` |
| OSC 7 working directory | yes: stored, used in tab titles, session save, and the directory of new tabs and splits (local host only) | yes | no; not in escape list | `scorecard_working_directory`; `inherit_dir.rs` tests; `terminal_manager.rs` `new_tab_opens_in_*` |
| OSC 133 prompt marks | yes: A/B/C/D recorded, used by `ulttyctl last-command`, events, jump to prompt, and select last output (only when it fits on screen) | yes, with `click_events` and `cl=` extensions | no | `tests/command_marks.rs`; `scorecard_prompt_marks` |
| Title stack (XTWINOPS 22/23) | yes, bounded | unverified in docs | yes | `conformance.rs` `title_stack_*` |
| DECSCUSR cursor shapes | yes: block, underline, bar, steady or blinking | yes | yes | `term_handler.rs` DECSCUSR arms; `renderer/cursor.rs` |
| Colour queries OSC 4/10/11/12 | partial: OSC 4 query and set; 10/11/12 query only; no setting fg/bg/cursor | yes | yes; query and set | `conformance.rs` `dynamic_colour_queries_*`, `osc_4_*`; `osc.rs` |
| Light/dark reports (2031, `CSI ? 996 n`) | yes | yes (version unverified) | no entry | `scorecard_colour_scheme_updates` |
| XTVERSION, DA1, DA2, DECRQM, DECRQSS, XTGETTCAP | yes | DA and XTVERSION unverified in docs | DA1 yes; XTVERSION no entry | `conformance.rs` `device_attributes_tell_one_story`, `xtgettcap_*`, `decrqss_*`; `scorecard_terminal_name` |
| Unicode width, mode 2027 | yes | yes; Unicode 17, grapheme conformance in 1.3 | partial; Unicode 17 widths, no 2027 | `scorecard_grapheme_clusters`; `grid/cluster.rs` |
| Grapheme clusters | yes; stored per cell, ZWJ and flags as one glyph | yes | unverified | `grid/cluster.rs`; `renderer/emoji.rs` |
| Right-to-left and bidi | no | partial; some RTL scripts, LTR only fully supported | no | no bidi code in `ultty-vt` or the renderer |
| Notifications OSC 9, 777 | yes | yes | no | `scorecard_notifications`; `app/actions.rs` `show_notification` |
| Progress OSC 9;4 | yes; shown in the tab title band | yes; native progress bar | no | `scorecard_progress`; `renderer/tabbar.rs` |
| Kitty text sizing (OSC 66) | partial: text printed, size ignored | partial; parsed only | no | `osc.rs` `"66"` arm |

### Graphics

| Feature | Ultty | Ghostty 1.4 | Alacritty 0.16 | Ultty evidence |
|---|---|---|---|---|
| Kitty graphics protocol | no; APC `G` consumed and dropped | yes; animation, relative placement | no | `ansi/osc.rs` APC handling; scorecard row "No" |
| Sixel | yes | no; maintainer declined it | no | `scorecard_sixel`; `conformance.rs` `sixel_images_span_cells_*`; `renderer/image.rs` |
| iTerm2 images (OSC 1337) | no | no; parsed only | no | no OSC 1337 handler |

### Text rendering

| Feature | Ultty | Ghostty 1.4 | Alacritty 0.16 | Ultty evidence |
|---|---|---|---|---|
| Font fallback | yes; system fonts via cosmic-text | yes; size-adjusted | yes; system font library | `renderer/text.rs`; `docs/configuration.md` "Emoji and accents" |
| Ligatures | yes: `font.ligatures` switch plus `font.features`; ASCII punctuation runs | yes; `font-feature` | no | `config.rs` `FontConfig::ligatures`, `features` |
| Variable fonts | no | yes; `font-variation` axes | no | no axis config in `FontConfig` |
| Emoji and colour fonts | yes; VS15/VS16 handling, configurable emoji font | yes; Noto emoji embedded | yes | `renderer/emoji.rs`; `font.emoji` |
| Box drawing, blocks, Powerline drawn natively | yes | yes; also auto-sized Nerd Font symbols | yes; `font.builtin_box_drawing` | `renderer/block_chars.rs`, `renderer/powerline.rs` |
| Anti-aliasing | partial: grayscale only; subpixel masks treated as mono | grayscale; `alpha-blending` modes | follows the platform font library (unverified) | `renderer/atlas.rs` `SwashContent::SubpixelMask` |
| Font features config | yes; `font.features` | yes; `font-feature` | no | `config.rs` `parse_font_features`; `docs/configuration.md` "OpenType features" |
| Per-style fonts (bold, italic) | yes; `font.bold_family`, `italic_family`, `bold_italic_family` | yes; `font-family-bold` etc. | yes; `font.bold`, `italic`, `bold_italic` | `renderer/text.rs` `FontSelection` |
| Codepoint-to-font map | yes; `[[font.codepoint_map]]` | yes; `font-codepoint-map` | no | `config.rs` `parse_codepoint_map`; `renderer/text.rs` `FontSelection::mapped` |
| Cell or glyph offsets | partial: `line_height` only | yes; `adjust-*` options | yes; `font.offset`, `glyph_offset` | `FontConfig::line_height` |
| Live font changes | yes: size via keys; family, size, features and line height on config reload | yes | yes | `renderer/text.rs` `apply_font_config`; `docs/configuration.md` "Changing fonts without a restart" |

### Input

| Feature | Ultty | Ghostty 1.4 | Alacritty 0.16 | Ultty evidence |
|---|---|---|---|---|
| IME and preedit | yes via winit: preedit shown, cursor area set | yes; macOS fixes in 1.3 and 1.4; GTK | partial; X11 and macOS fixes listed, Wayland not documented | `app/input.rs` `Ime::Preedit`/`Commit`; `app/render.rs` `set_ime_cursor_area` |
| Dead keys, compose | yes via winit text; dead key encoding tested | yes (GTK fixes in 1.3/1.4) | unverified | `keys.rs` `WinitKey::Dead` test, composed `é` test |
| Keybinding config | partial: fixed set of actions, `mod+key` only; no sequences, key tables or mouse bindings | yes; sequences, key tables, chains, `global:` | yes; modes, chars, commands, mouse bindings | `config.rs` `KeybindingConfig`, `parse_keybinding` |

### UI

| Feature | Ultty | Ghostty 1.4 | Alacritty 0.16 | Ultty evidence |
|---|---|---|---|---|
| Tabs | yes | yes | no (native tabs on macOS only) | `tabs.rs`; `terminal_manager.rs` `create_tab` |
| Splits | yes; nested | yes; draggable | no | `pane_layout.rs`; `terminal_manager.rs` `split_pane` |
| Multiple windows | partial: one window per process; "New window" spawns a new process | yes | yes; `CreateNewWindow`, `msg create-window` | `app/input.rs` `MenuItem::NewWindow` |
| Native vs self-drawn UI | self-drawn; no decorations, own tab bar | native GTK4/libadwaita and AppKit | native decorations, no chrome | `app/mod.rs` `with_decorations(false)`; `renderer/tabbar.rs` |
| Quick terminal | no | yes; macOS and Wayland | no | none |
| Search in scrollback | partial: in copy mode; smart case, optional regex, matches across wrapped lines, all matches highlighted with a count; no search bar | yes since 1.3 | yes; regex | `vim/search.rs`; `app/mod.rs` `perform_search` |
| URL detection | yes; regex, Ctrl+click | yes; `link-url`, previews | yes; configurable regex hints | `url.rs` |
| Vi or copy mode | yes; motions, search, char/line/block selection | no | yes | `vim/mod.rs` (tests in module) |
| Selection: word, line, block | yes; double-click word, triple-click line, Alt for block | yes; plus select command output | yes; plus `semantic_escape_chars` | `app/input.rs` selection; `grid/terminal.rs` `SelectionMode`; `tests/selection.rs` |
| Configurable word characters | no | yes; `selection-word-chars` | yes | `app/input.rs` `is_word_char` is fixed |
| Command palette | no | yes | no | none |
| Inspector or debug overlay | partial: native-protocol status overlay, debug dump; no VT inspector | yes; terminal inspector | partial; render timer, damage highlight | `Ctrl+Shift+I`, `keybindings.debug_dump` |
| Scrollbar | no | yes since 1.3 | no | none in renderer |

### Config

| Feature | Ultty | Ghostty 1.4 | Alacritty 0.16 | Ultty evidence |
|---|---|---|---|---|
| Format | TOML | key = value | TOML, with imports | `config.rs` |
| Live reload | partial: file watched; `behavior.accessibility` needs restart | on request (keybind or menu) | yes; `live_config_reload` | `config_watcher.rs`; `tests/config_reload.rs` |
| Bundled themes | 6 | hundreds (356 added in 1.4); light/dark pairs | none bundled (separate repo) | `config.rs` `ColorConfig::default` |
| Per-platform options | no | yes; `macos-*`, `gtk-*` | yes | none in `config.rs` |
| CLI overrides | partial: `-e` only (added after this audit); no config overrides | yes; `--key=value` | yes; `-o`, `-e`, `--working-directory` | `main.rs` `invocation` |
| Launch profiles | yes; tabs and panes with commands and directories | no | no | `config.rs` `ProfilesConfig` |

### Platforms

| Feature | Ultty | Ghostty 1.4 | Alacritty 0.16 | Ultty evidence |
|---|---|---|---|---|
| Linux X11 and Wayland | yes | yes | yes | winit; T4 runs on both |
| macOS | partial: code exists; no release builds, not in CI | yes | yes | `platform/macos.rs`; README platform status |
| Windows | no | no; roadmap dropped in 1.4 | yes; ConPTY | `platform/mod.rs` has Linux and macOS only |
| BSD | no | yes; FreeBSD | yes | no BSD `cfg` in `platform/` |
| Packaging | partial: Linux x86_64 tarball, `.deb`, `.rpm`, Homebrew formula, `install.sh` | macOS app, distro packages, Snap and Flatpak | release binaries for macOS and Windows; distro packages | `dist/`, `install.sh`, `docs/release-artifacts.md` |

### Shell integration

| Feature | Ultty | Ghostty 1.4 | Alacritty 0.16 | Ultty evidence |
|---|---|---|---|---|
| Prompt marks | partial: recorded; scripts for bash and zsh, sourced by hand | yes; auto-injected for bash, zsh, fish, elvish, nushell | no | `dist/shell-integration/`; `tests/command_marks.rs` |
| Jump to prompt | yes; `jump_prev_prompt`, `jump_next_prompt` (Ctrl+Shift+PageUp/PageDown); `select_last_output` (Ctrl+Shift+O) | yes | no | `Terminal::jump_to_prompt`; `tests/command_marks.rs` |
| cwd inheritance for new tabs and splits | yes; OSC 7 (local host), else the foreground process's `/proc` cwd on Linux; `behavior.inherit_working_directory` | yes; separate tab and split options | yes for new windows (unverified in docs) | `inherit_dir.rs`; `terminal_manager.rs` `inherited_directory` |
| ssh and sudo helpers | partial: in-band native protocol over ssh; `xterm-ultty` terminfo copied by hand | yes; `+ssh` terminfo and env, sudo wrapper | no | `docs/configuration.md` terminal type; ADR 0018 |

### Accessibility

| Feature | Ultty | Ghostty 1.4 | Alacritty 0.16 | Ultty evidence |
|---|---|---|---|---|
| Screen reader | yes on Linux (AT-SPI via AccessKit): pane text, caret, native regions, live announcements | partial; macOS only, read-only | no | `a11y/tree.rs` `Role::Terminal`; `a11y/live.rs`; `docs/accessibility-test.md` |

### Other

| Feature | Ultty | Ghostty 1.4 | Alacritty 0.16 | Ultty evidence |
|---|---|---|---|---|
| Scrollback | 10,000 lines default, up to 1,000,000; applies on reload | 50 MB default; byte or line limits | 10,000 default, 100,000 max | `config.rs` `scrollback_lines`, `MAX_SCROLLBACK_LINES` |
| Background opacity | yes | yes | yes | `behavior.opacity`; `renderer/panes.rs` |
| Background blur | no | yes | yes (KDE Wayland, macOS) | none |
| Background image | no | yes | no | none |
| Bell | partial: visual flash and urgency hint; no sound or command | yes; system, audio, attention, title, border | yes; visual, colour, command | `behavior.bell_visual`, `bell_urgent` |
| Session restore | partial: opt-in; tabs, splits, directories; no scrollback | macOS only | no | `session.rs`; `app/mod.rs` `restore_session` |
| Multiplexing or daemon | no | no | unverified | none |
| IPC and automation | yes; `ulttyctl` with ~25 commands, events, MCP server | AppleScript (macOS); `+new-window`, `+new-tab` | `alacritty msg create-window`, `config`, `get-config` | `bin/ulttyctl.rs`; `control/server.rs`; `control/mcp.rs` |
| Native app protocol | yes; socket and in-band | no | no | `ultty-protocol`; `docs/PROTOCOL_SPEC.md` |

## Rows not verified

- Ultty dead keys and compose: covered by a key-encoding unit test, not by a
  run with a real input method.
- Ultty IME: code path read, not run. No headless IME test exists.
- Ghostty: SGR-pixel mouse, title stack, XTVERSION and DA replies, OSC
  10/11/12 queries, the version that added mode 2031, whether config
  reloads without being asked, Linux accessibility
  (none found), and its anti-aliasing modes. Ghostty's VT reference says it
  lists only some of the supported sequences.
- Alacritty: grapheme clustering, dead keys and compose, Wayland IME,
  anti-aliasing, cwd inheritance for new windows, and whether daemon mode
  exists. Its escape-support doc was read at tag v0.16.1; the file is no
  longer on `master`.

## Sources

- Ghostty: <https://ghostty.org/docs/install/release-notes/1-4-0>,
  <https://ghostty.org/docs/install/release-notes/1-3-0>,
  <https://ghostty.org/docs/install/release-notes/1-2-0>,
  <https://ghostty.org/docs/config/reference>,
  <https://ghostty.org/docs/features>, <https://ghostty.org/docs/vt/reference>,
  <https://ghostty.org/docs/features/shell-integration>,
  <https://github.com/ghostty-org/ghostty/discussions/2496> (sixel)
- Alacritty: <https://alacritty.org/config-alacritty.html>,
  <https://alacritty.org/misc-alacritty-escapes.html>,
  <https://raw.githubusercontent.com/alacritty/alacritty/v0.16.1/docs/escape_support.md>,
  <https://github.com/alacritty/alacritty/blob/master/CHANGELOG.md>,
  <https://github.com/alacritty/alacritty/blob/master/docs/features.md>,
  <https://github.com/alacritty/alacritty/releases>,
  <https://github.com/alacritty/alacritty/issues/8792>
