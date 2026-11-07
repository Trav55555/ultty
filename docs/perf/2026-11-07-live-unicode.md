# Live unicode render cost - 2026-11-07

On the AWS comparison box (g4dn.xlarge, Tesla T4, X11, 120x40, DejaVu Sans
Mono 11, `scripts/bench/compare.sh` with `ubench-feed`), live throughput for
the `unicode-mixed` corpus was 0.95 MiB/s against Alacritty's 43 and GNOME
Terminal's 29. ASCII was on par (56 against 48 and 57). Headless parse and
grid for `unicode-mixed` runs at about 38 MiB/s on `integration-wave1`, so
almost all of the live cost is outside the VT core.

Finding: the renderer shaped every multi-codepoint grapheme (combining
marks, ZWJ emoji, flags, skin-tone modifiers) again each time it rebuilt a
row. Output scrolling past rebuilds every row every frame, so a full
`unicode-mixed` screen spent 80% of its frame in cosmic-text shaping. Those
graphemes are now cached per face by text, as ligature runs already were.
Rendering a scrolling `unicode-mixed` screen costs 5.2x fewer instructions,
and 3.7x less wall time on the local GPU. ASCII and CJK are unchanged.

The 0.95 MiB/s figure did **not** reproduce on this workstation. Here live
`unicode-mixed` throughput under Xvfb and lavapipe is 10-16 MiB/s both before
and after the fix, because frames are rarely drawn during a feed (see
[Live run](#live-run-xvfb--lavapipe)). Whether the fix closes the AWS gap
needs a real-GPU re-run (see [Next steps](#next-steps)).

Claim classes: `micro` (renderer instruction counts and headless frame wall
time) and `live-swrast` (Xvfb). No `live-realgpu` claim is made.

## Environment

| Field | Value |
|---|---|
| Base | `integration-wave1` at `5d73b8d` |
| Toolchain | rustc 1.98.1 (Homebrew), bench and release profiles |
| Build | isolated `RUSTFLAGS="--cfg wt_liveuni"` |
| Machine | Ryzen 5 7600 (12 threads), Radeon RX 7900 XT, load average 8-10 (shared) |
| Callgrind | valgrind 3.27.1, `--toggle-collect='*render_to_view*'` |

## The new bench case

`render_corpus` draws the same screen on every iteration. After the first
frame every row comes from the row vertex cache (`text_cache.rs`), so it
cannot show what a frame of new output costs. The new
`render_scrolling@<adapter>/<corpus>/120x40` group (`crates/ultty/benches/render.rs`)
cycles through 16 screens of a corpus, each one line further on. Every row
differs from the row it replaces, so every frame rebuilds every row, as when
a program streams output into a tab. It covers `ascii`, `unicode-mixed` and
`cjk`.

```bash
cargo bench -p ultty --bench render -- render_scrolling
```

## Profile

`perf` is not usable here (`perf_event_paranoid` is 4), so the profile comes
from callgrind over the bench binary in profile mode, counting only inside
`Renderer::render_to_view`:

```bash
valgrind --tool=callgrind --toggle-collect='*render_to_view*' \
  <render bench binary> --bench 'render_scrolling.*/unicode-mixed/' --profile-time 60
```

Inclusive share of a `unicode-mixed` frame before the fix (347 frames):

| Function | Share |
|---|---:|
| `TextRenderer::build_text_vertices_with_offset` | 95.5% |
| `TextRenderer::shape_text` (grapheme clusters only) | 80.2% |
| `cosmic_text::shape::shape_fallback` | 63.7% |
| `harfrust::hb_ot_shape_plan_t::new` | 36.3% |
| `FontFallbackIter::next` | 9.4% |
| `GlyphAtlas::get_glyph` | 6.6% |

About 150 grapheme clusters per frame each cost about 104k instructions:
cosmic-text runs font fallback and builds a fresh HarfRust shape plan on
every call. Single characters were already cached by `(char, style)`, and
ligature runs by text. Only the multi-codepoint path in `build_row_vertices`
had no cache.

Ruled out:

- **Glyph atlas misses or evictions.** With a miss logger on
  `GlyphAtlas::get_glyph`, a 2-second profile run rasterized 210 glyphs, all
  on the first frames, and none again. The atlas never filled or reset.
- **Per-frame font fallback for single characters.** These hit the
  `shaping_cache`, so fallback appears only under cluster shaping.
- **The PTY reader falling back to per-byte parsing.** The live reader uses
  the same `feed_with_ascii_spans` path as the headless benchmark
  (`event_loop.rs::parse_into`), which measures 38 MiB/s.

## Results

### Instructions per frame (deterministic)

`render_scrolling`, 120x40, callgrind instructions inside `render_to_view`
divided by frames. This includes the driver's CPU work for the frame.

| Corpus | Before | After | Change |
|---|---:|---:|---:|
| `ascii` | 5,781,838 | 5,720,331 | -1% |
| `unicode-mixed` | 19,745,096 | 3,783,603 | **-81%** |
| `cjk` | 2,281,166 | 2,309,594 | +1% |

### Wall time per frame (best of N, interleaved)

Criterion point estimates. Before and after alternate within each round, and
the table shows the best round of each.

| Adapter | Corpus | Before | After |
|---|---|---:|---:|
| RX 7900 XT, Vulkan (5 rounds) | `ascii` | 615 µs | 618 µs |
| | `unicode-mixed` | 1.785 ms | 478 µs |
| | `cjk` | 281 µs | 298 µs |
| radeonsi, GL (3 rounds) | `ascii` | 956 µs | 933 µs |
| | `unicode-mixed` | 2.679 ms | 845 µs |
| | `cjk` | 703 µs | 693 µs |
| Vulkan, DejaVu fonts only (1 round) | `unicode-mixed` | 4.018 ms | 442 µs |

The last row runs with `FONTCONFIG_FILE` pointing at a config that lists only
`/usr/share/fonts/truetype/dejavu`, standing in for a machine with few
fallback fonts. With no emoji or CJK font, cluster shaping walks fallback to
the end every time, so the uncached cost was 2.3x the full-font case. A
minimal cloud image is likely closer to this row than to the workstation.

### Live run (Xvfb + lavapipe)

`ubench-feed` inside a release `ultty` on Xvfb with lavapipe (114x50, the
default window, DejaVu Sans Mono 11, ligatures off), 4 MiB corpora, three
passes each:

| Arm | `ascii` MiB/s | `unicode-mixed` MiB/s |
|---|---|---|
| before | 53.9, 72.2, 53.0 | 13.8, 15.9, 16.2 |
| after | 52.0, 100.7, 81.0 | 10.4, 19.2, 14.6 |
| before, pinned to one CPU | 35.5, 31.2, 31.2 | 10.4, 10.5, 13.0 |
| after, pinned to one CPU | 35.5, 37.5, 31.5 | 12.6, 14.8, 12.9 |

With 16 MiB corpora in `compare.sh` order (ascii, sgr-dense, unicode-mixed),
the before arm gave `unicode-mixed` 12.8, 12.2 and 15.1 MiB/s. No run came
near 0.95 MiB/s.

These runs cannot show the fix. With `ULTTY_TRACE_PIPELINE=1`, a 4 MiB pass
draws about 16 frames, at 5-13 ms each (lavapipe rasterizing). The
renderer's share of the feed is small, and the swrast frame time is mostly
rasterization, not text building.

## Why render cost can collapse live throughput

`App::render_single_pane` (`app/render.rs`) holds the focused terminal's
lock (`event_loop.terminal()`) for the whole frame: text building, the
submit and the present at the end of `render_with_overlay`. The PTY reader thread
(`event_loop.rs::drain`) needs the same lock to parse each read. While a
frame builds, parsing stops. When frame cost approaches the 16 ms frame
interval, `should_render` schedules frames back to back. The reader then
gets the lock only between frames, and the mutex makes no fairness
guarantee. Throughput falls off a cliff rather than degrading in proportion.

ASCII frames on the AWS box stay well under the interval. The uncached
`unicode-mixed` frame was 3-9x dearer here, and probably dearer still on a
2.5 GHz Xeon with fewer fallback fonts. That fits the reported collapse, but
it has not been measured on that box.

## Correctness

Screenshots taken with `ulttyctl screenshot` under Xvfb and lavapipe, with
ASCII, CJK, emoji (including ZWJ and skin tone), combining sequences, Greek,
Cyrillic, Hebrew and Arabic, bold and italic graphemes, and repeated
graphemes on one row:

- before: `shot-before.png`
- after: `shot-after.png`

The two PNGs are byte-identical. Both show pre-existing rendering issues
that this change neither causes nor fixes:

- Some combining marks (`é̃`, `ụ̃`) draw their second mark offset into the
  next cell.
- The family ZWJ sequence draws as a box.

## Remaining frame cost (`unicode-mixed`, after)

- SipHash in `HashMap` lookups (atlas `CacheKey`, `shaping_cache`): about 32%
  of the 3.8M instructions per frame. A faster hasher would also help ASCII.
  It needs a direct dependency (`rustc-hash` or `foldhash`, both already in
  the lockfile).
- `build_row_vertices` fills a per-row `indices` vector that it then throws
  away (the draw uses the shared quad index list): about 4%.
- `memcpy`: about 10%.

## Next steps

1. Re-run `scripts/bench/compare.sh --session x11 --corpora ascii,unicode-mixed`
   on the g4dn box with this branch to confirm the live gap closes. Record
   `fc-list | wc -l` there and whether an emoji font is installed.
2. If a gap remains, set `ULTTY_TRACE_PIPELINE=1` on the same run and check
   `pipeline_render_frame` times against the 16 ms interval. If frames are
   still close to it, move frame building off the terminal lock: snapshot the
   visible rows under the lock, then build and submit without it. That is a
   design change in `app/render.rs` and is not part of this fix.
