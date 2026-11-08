# Glyph atlas that grows (2026-11-08)

Plan item D3. The [idle-footprint note](2026-11-07-idle-footprint.md) found
the 2048x2048 RGBA8 glyph atlas was 16 MiB of the 22-24 MiB of GPU memory,
most of it empty for a terminal using one face.

The atlas now starts at 1024x1024 (4 MiB) and doubles, up to 2048x2048,
when a frame finds it full. Growth happens at the start of the next
frame, as eviction already did: every glyph is dropped and re-rasterized
on use, and the atlas epoch changes, because UVs are fractions of the
texture size. At 2048 a full atlas is emptied, as before. A full glyph
cache (4096 entries) still empties the atlas without growing it.

Growing exposed that cached native-session geometry was reused after the
atlas emptied, with UVs from the old layout. That cache now records the
atlas epoch it was built in and is rebuilt when it changes.

## Measured

Workstation, Xvfb, lavapipe forced (`VK_DRIVER_FILES=.../lvp_icd.json`, so
GPU memory is host memory and shows as `memfd` mappings), release build,
after `cat` of the changelog, README and protocol spec, then `ls`. Two
runs each:

| | GPU memory (`memfd` RSS) | process RSS |
|---|---:|---:|
| before | 22.9 MB | 121.2-121.3 MB |
| after | 11.4-11.9 MB | 111.1-113.4 MB |

Printing 6,400 distinct CJK characters, 1,600 per screen, grew the atlas
once, to 2048, after about 4,400 glyphs; text drawn afterwards was
correct (screenshot checked).

## Not done

- RSS against Alacritty on the T4 (D3's acceptance) needs the T4 run.
- An R8 atlas for monochrome glyphs, with colour glyphs apart, would cut
  the common case to a quarter again (1 MiB at start).
- The font database was already measured as small (0.4 MiB resident).
