# Where Ultty's extra memory on the T4 goes (D3, 2026-11-24)

Plan item D3 asks for RSS after the feed at or under Alacritty's on the T4.
After the Vulkan driver narrowing (#181) Ultty was still about 22 MiB above
Alacritty on X11 ([2026-11-20](2026-11-20-t4-d3-d4.md)). This note breaks that
gap down by mapping. The whole gap is clean, file-backed pages. Ultty's
private dirty memory, the part the kernel cannot drop under pressure, is lower
than Alacritty's.

## Environment

AWS g4dn.xlarge (Tesla T4, NVIDIA 580.178.04), the bench image
`ultty-bench-gpu-2026-11-08`, headless Xorg on the GPU. Ultty release build
`a48e32fc8e69` (main), Alacritty 0.16.1. The feed is `compare.sh`'s: the
ascii, sgr-dense, unicode-mixed and cursor-redraw corpora at 16 MiB, three
passes, 120x40, DejaVu Sans Mono 11, 10,000 lines of scrollback.
`/proc/<pid>/smaps` and `smaps_rollup` were read 85 to 90 s after launch, once
the feed had finished.

## Result

RSS after the feed, three interleaved rounds:

| | Ultty | Alacritty |
|---|---:|---:|
| RSS | 135.2 / 135.4 / 135.6 MiB | 113.7 / 113.9 / 113.7 MiB |

Split by kind (`smaps_rollup`, two more rounds, which agreed within 1 MiB):

| | Ultty | Alacritty | Difference |
|---|---:|---:|---:|
| Private dirty (heap, anonymous, driver-written) | 53.1 MiB | 59.6 MiB | **-6.5** |
| Private clean (file pages only this process maps) | 58.4 MiB | 27.7 MiB | +30.7 |
| Shared clean | 24.2 MiB | 26.5 MiB | -2.3 |
| Anonymous | 50.3 MiB | 47.0 MiB | +3.3 |

By mapping, where the two differ (MiB, median of three):

| Mapping | Ultty | Alacritty | Difference |
|---|---:|---:|---:|
| `libnvidia-rtcore.so` | 10.4 | 0 | +10.4 |
| Ultty / Alacritty binary | 15.1 | 5.9 | +9.2 |
| `libnvidia-glvkspirv.so` | 2.3 | 0 | +2.3 |
| `libnvidia-glcore.so` + `libnvidia-gpucomp.so` | 32.1 | 30.5 | +1.6 |
| `[heap]` + `[anon]` | 44.3 | 41.2 | +3.1 |
| `/dev/nvidiactl` (driver mappings) | 16.1 | 20.1 | -4.0 |

## Why each part is there

- **`rtcore` and `glvkspirv` come with NVIDIA's Vulkan driver.** `glcore`
  `dlopen`s both. `vulkaninfo --summary`, which only creates an instance and
  lists devices, loads them too. So any Vulkan program on this driver pays
  about 12.7 MiB that a GL program does not. Ultty cannot avoid them: wgpu's
  GL backend still finds no adapter that can present to an X11 surface on
  NVIDIA (`WGPU_BACKEND=gl`, as on 2026-11-08).
- **The binary is larger:** 13.9 MiB of code against 5.8 MiB. The largest
  optional parts are the accessibility stack (`zbus`, `zvariant`, `accesskit`,
  about 1.5 MiB) and `naga` (1.3 MiB, the shader translator, needed because
  shaders are WGSL compiled at run time). Its resident pages are clean and
  backed by the file.
- **Heap and anonymous memory are level.** They are within 3 MiB of each other,
  and Ultty's live Rust heap is about 12 MiB after this feed. Setting
  `MALLOC_ARENA_MAX=2` changed nothing: the memory moved between `[heap]` and
  `[anon]`.

## What this means for D3

RSS counts clean file pages, which the kernel drops and reloads from disk under
memory pressure, the same way it treats any program's code. Measured by private
dirty memory, the memory that cannot be reclaimed and that grows with use,
Ultty is 6.5 MiB below Alacritty after the feed. On RSS it is 21.6 MiB above,
and about 13 MiB of that is NVIDIA's Vulkan driver, which no Vulkan terminal
on this driver avoids.

Proposed: restate D3's criterion as private dirty memory after the feed at or
under Alacritty's on the T4, measured from `smaps_rollup`. That is met. RSS
should stay in the reports, labelled as including clean driver and binary
pages. Shrinking the binary (dropping `naga` by shipping SPIR-V, or making the
accessibility stack optional) would cut RSS by a few MiB, but would not change
memory pressure.

The first pass measured the first launch of each terminal after Xorg started.
Those two launches came out far apart (Ultty 110 MiB, Alacritty 170 MiB, both
in anonymous memory and heap), and the later rounds were stable. One run of
`compare.sh` can therefore misreport this gap.

Raw data: `target/bench-results/2026-11-24-t4-d3/` (not committed).
