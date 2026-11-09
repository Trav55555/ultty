# Performance Artifact Conventions

Use this directory for measurements that support Ultty performance work. Every
artifact should make the result reproducible, say what kind of claim the data
supports, and preserve enough raw detail for later comparison.

## Artifact Types

| Type | Use for | Example |
|------|---------|---------|
| Baseline | Repeatable Criterion or benchmark-suite runs that support README or release claims. | [`2026-05-14-baseline.md`](./2026-05-14-baseline.md) |
| Live stress | End-to-end terminal runs through a real tab, PTY, parser, grid, and compositor. | [`2026-05-14-live-pipeline-trace.md`](./2026-05-14-live-pipeline-trace.md) |
| Investigation | A targeted hot-path note with evidence, hypotheses, and a recommendation. | `YYYY-MM-DD-<topic>.md` |
| Design note | A performance-sensitive design decision that is not yet benchmarked. | [`styled-native-text.md`](./styled-native-text.md) |

## Current Release Gates

Use these gates before publishing performance or chrome claims:

```bash
cargo fmt --all -- --check
cargo check --workspace
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
scripts/live-pipeline-trace.sh /tmp/ultty-live-pipeline-trace
scripts/softwrap-pty-probe.sh /tmp/ultty-softwrap-pty-probe
scripts/screenshot-smoke.sh /tmp/ultty-screenshot-smoke
scripts/native-richtext-smoke.sh /tmp/ultty-native-richtext-smoke
scripts/append-content-smoke.sh /tmp/ultty-append-content-smoke
```

The live pipeline and soft-wrap probes require a GUI-capable Ultty run. The
renderer screenshot smokes also require a running Ultty control socket and
capture renderer output through `ulttyctl screenshot`; see
[`../visual-regression.md`](../visual-regression.md).

`scripts/live-pipeline-trace.sh` passes a longer stress timeout to
`scripts/perf-smoke.sh` because live PTY workloads can exceed the generic
60-second smoke default on slower or noisy workstations. Override it with
`ULTTY_TRACE_TIMEOUT_SECS=<seconds>` when collecting a targeted investigation.

The current dated evidence set is:

- [`2026-11-07-vt-corpus-baseline.md`](./2026-11-07-vt-corpus-baseline.md) for
  headless VT throughput and instructions per byte over the CRLF corpora,
  measured heap bytes, and the A/A noise floor (supersedes the memory section
  of the 2026-05-14 baseline).
- [`2026-11-07-live-unicode.md`](./2026-11-07-live-unicode.md) for the
  renderer's per-frame cost on scrolling non-ASCII output (grapheme shaping
  cache) and the `render_scrolling` bench.
- [`2026-11-07-unicode-throughput.md`](./2026-11-07-unicode-throughput.md) for
  the non-ASCII and scrolling instruction cuts, and corrected instruction
  counts (the earlier gate undercounted).
- [`2026-11-08-glyph-atlas.md`](./2026-11-08-glyph-atlas.md) for the glyph
  atlas starting at 1024x1024 and growing (GPU memory 22.9 to 11.4 MB).
- [`2026-11-08-control-latency.md`](./2026-11-08-control-latency.md) for
  control request and event delivery latency, and background tabs no longer
  redrawing the window.
- [`2026-11-08-t4-comparison.md`](./2026-11-08-t4-comparison.md) for the
  T4 rerun with keypress-to-pixel latency, where startup goes, and the
  NVIDIA driver's idle wakeups.
- [`2026-11-08-agent-soak.md`](./2026-11-08-agent-soak.md) for the ten-hour
  agent-session soak: frame time met, memory inconclusive (host reclaimed pages).
- [`2026-11-07-idle-footprint.md`](./2026-11-07-idle-footprint.md) for idle
  wakeups (now none with the cursor steady), the memory breakdown, and the
  startup stage split.
- [`2026-11-08-nvidia-idle-wakeups.md`](./2026-11-08-nvidia-idle-wakeups.md)
  for the ~113 idle wakeups/s on NVIDIA, which come from a driver thread, not
  from Ultty.
- [`2026-11-08-accessibility-adapter.md`](./2026-11-08-accessibility-adapter.md)
  for the AccessKit adapter's cost: none at startup, two idle wakeups/s from
  zbus's blocking pool, hence `accessibility = false` by default.
- [`2026-11-08-live-throughput.md`](./2026-11-08-live-throughput.md) for the
  live flood profile (the PTY I/O thread is CPU-bound on parse, grid and
  scrollback encoding), the parser, scrollback and frame-cost cuts, and the
  `pty_flood` bench.
- [`2026-05-29-current-readiness.md`](./2026-05-29-current-readiness.md) for
  current benchmark refresh, fixed live-pipeline evidence, soft-wrap evidence,
  and process-level idle sample.
- [`2026-05-14-baseline.md`](./2026-05-14-baseline.md) for repeatable benchmark
  baselines.
- [`2026-05-14-ascii-span.md`](./2026-05-14-ascii-span.md) for parser/grid
  hot-path changes.
- [`2026-05-14-live-pipeline-trace.md`](./2026-05-14-live-pipeline-trace.md)
  for live PTY, parser/grid, render, and readiness wakeup evidence.

## Benchmark Suite

The suite has three tiers, and each claims only what it can support:

| Tier | Oracle | Tool | Use |
|------|--------|------|-----|
| **D** deterministic | instruction counts (callgrind), heap bytes (counting allocator) | `scripts/bench/icount-gate.sh`, `cargo test -p ultty --bench memory` | regression gate; load-insensitive |
| **A** local A/B | wall clock, interleaved blocks, bootstrap CI | `scripts/bench/run.sh vt --ab A B` / `--aa` | perf PRs, with a measured noise floor |
| **R** release evidence | live, real GPU, repeated | `scripts/bench/compare.sh`, `scripts/bench-aws.sh` | README and release claims, comparisons |

Every result is labelled with a **claim class**: `micro` (parser only),
`headless-e2e` (parse + grid, no PTY or renderer), `live-realgpu`,
`live-swrast` (Xvfb with llvmpipe/lavapipe), or `comparative`. Never compare a
`live-swrast` number with a `live-realgpu` one: on this workstation live
throughput under lavapipe is about 30x below headless, so it measures the
software rasterizer.

### Rules learned the hard way

1. **Corpora are CRLF.** A bare `\n` staircases the cursor and changed
   headless throughput by about 45%. The corpus generator asserts CRLF, and
   `ultty-bench` and `ubench-feed` refuse external files with a bare LF.
2. **Benchmark the production entry point** (`feed_with_ascii_spans` into a
   `TerminalHandler`), not a bench-local copy.
3. **Never report one number.** Median, IQR, min and n; comparisons use
   interleaved, shuffled blocks and a CI on the ratio of medians.
4. **Record the environment**: revision and dirty flag, rustc, CPU, governor,
   EPP, boost, pin set, load at start and end, adapter, session, cell geometry.
5. **Measure the noise floor first.** An A/A run (the same binary as both
   arms) gives the session's minimum detectable effect (MDE); verdicts use
   `max(threshold, 2 x MDE)`. With load above 12 the scripts refuse or mark
   results noisy.

### Corpora

`ultty-bench corpus --out DIR [--size-mib 16]` writes eleven deterministic
corpora and a `manifest.json` with SHA-256s (`tools/ultty-bench/src/corpus.rs`;
1 MiB hashes are pinned by a unit test): `ascii`, `sgr-dense`,
`unicode-mixed`, `cjk`, `emoji-zwj`, `combining`, `cursor-redraw` (htop/vim
style CUP redraws), `scroll` (short lines), `scroll-region` (DECSTBM with
IND/RI), `alt-screen` (`?1049h` frames in `?2026` sync updates), `wide-wrap`
(200-4000 character lines). Cursor-addressed corpora are authored for 120x40.

### Headless throughput (`tp.vt`, `tp.parse`)

```bash
cargo build --release -p ultty-bench
taskset -c 5,11 target/release/ultty-bench vt --suite vt,parse --runs 10 \
  --geometry 80x24,120x40 --screen main,alt --out results.jsonl
python3 -I scripts/bench/analyze.py results.jsonl --markdown

# Noise floor, then an A/B between two revisions (worktrees under target/bench-build/):
scripts/bench/run.sh vt --aa --runs 10 -- --corpus ascii,unicode-mixed
scripts/bench/run.sh vt --ab main HEAD --runs 10 -- --corpus ascii,unicode-mixed
```

Each sample is one JSON line:

```json
{"suite":"tp.vt","case":"sgr-dense/120x40/main","arm":"A","block":3,"value":98.2,
 "unit":"MiB/s","claim":"headless-e2e","corpus_sha256":"…","rev":"…","dirty":false,
 "env":{"cpu":"…","governor":"powersave","epp":"balance_performance","pin":"5,11",
        "load_start":9.6,"load_end":10.7,"rustc":"rustc 1.98.1 …","profile":"release"}}
```

`analyze.py` (stdlib only; run with `python3 -I`; `--self-test` checks it)
prints the per-case summary, or with two arms the ratio B/A, its paired
bootstrap 95% CI, the Mann-Whitney p-value and a verdict (`--aa` reports the
MDE, `--mde-from aa.jsonl` gates an A/B on it, `--neutral` words verdicts for
product comparisons, `--fail-on-regression` exits 3). Fewer than five samples
per arm get no verdict.

### Deterministic gate

```bash
scripts/bench/icount-gate.sh            # +3% instructions fails; then runs the memory budgets (+5%)
scripts/bench/icount-gate.sh --update   # rewrite bench/baselines/icount.json, in its own commit
```

Instruction counts are collected only inside `run_vt_pass` / `run_parse_pass`
over 1 MiB of each corpus at 120x40; repeated runs give identical parser counts and parse+grid counts within
0.35% (hash seeding varies). The
baseline records rustc and valgrind versions and refuses to compare across
them, so it is per toolchain (the committed one is from the Homebrew rustc on
the development box; regenerate it where the gate runs). It needs valgrind, so
it is not in CI; the memory budgets are (`cargo test -p ultty --bench memory`
in the Gitea test job).

### Criterion micro benches

`parser` (production path, 8 MiB corpora, non-ASCII included),
`vt_small_writes` (formerly `input_latency`; it never measured latency, and its
payloads now use CRLF), `layout` (`relayout_dirty_leaf`, `relayout_resize`, and
`cached_same_size`, the old clean-tree number kept as a floor), `render` (one
reused offscreen target; the adapter is printed and is part of every group
name, e.g. `render_filled@gpu-vulkan` or `@swrast-gl`), `pty_flood` (32 MiB
corpora written by `dd` into a raw-mode PTY and read by the real I/O thread,
no GPU), `grid`, and
`ultty-protocol`'s `codec` (every case now sets its own throughput). Use
`--save-baseline` / `--baseline` for local before/after comparisons, and treat
them as diagnosis rather than gates.

### Live and comparative (`tp.live`, `lat.k2p.external`, `startup.window`, `mem.rss.live`, `idle.*`, `tabs.scale`)

Terminal-neutral drivers in `tools/ultty-bench` run *inside* each terminal:

- `ubench-feed`: writes corpus files to `/dev/tty`, then sends `CSI 6n` and
  stops the clock at the reply, which proves the terminal consumed every byte
  (timing `cat` from a shell only shows when the producer finished). It checks
  `stty size` against `--expect-size` before measuring.
- `ubench-echo`: raw-mode echo of each key at a fixed cell, optionally under a
  DECSTBM flood (`--flood 10` MiB/s or `max`) that starts at the first `f`
  key, which the probes send after calibrating on a still screen.
- `ubench-k2p` (X11): XTest key injection, then GetImage polling of the echo
  cell until it changes. Measures injection to pixels in the X server's
  framebuffer, not photons; the median poll time is recorded as
  `resolution_us` (about 25 us locally). Under Xvfb there is no vsync.
- `scripts/bench/k2p-wayland.py` (sway): `wtype` plus `grim` polling. Each
  poll is a process spawn and a screencopy, so resolution is tens of
  milliseconds (`resolution_ms` is recorded); treat it as coarse.

`scripts/bench/compare.sh --session xvfb|x11|wayland` runs Ultty, Ghostty,
GNOME Terminal and Alacritty with matched settings: the same font family
(the resolved file and its hash are recorded), size and 120x40 cells (checked
with `stty size`; windows that come up a row off are resized until they
match), padding 0, ligatures off, 10,000 lines of scrollback (Ghostty's limit
is in bytes, so it is approximated at 8 bytes per cell), cursor blink off.
Ghostty runs with `gtk-single-instance = false` and no decorations; GNOME
Terminal runs under `dbus-run-session` with a private `XDG_CONFIG_HOME`
(dconf profile, `gtk.css` padding 0), and its memory and CPU are read from the
private `gnome-terminal-server`, not the client; Alacritty's tab scaling uses
windows in one process and is marked `"equivalent": false`. Rounds are
interleaved with a shuffled terminal order. It records throughput, startup
(exec to the titled window), RSS/PSS after the workload, idle CPU and context
switches over all threads (`/proc/<pid>/stat`, `/proc/<pid>/task/*/sched`),
latency idle and under flood, and RSS against tab count, then writes
`report.md` with Ultty-vs-each tables.

Sessions: `xvfb` is `live-swrast`. `x11` uses the X server in `$DISPLAY` (on
AWS, headless Xorg on the NVIDIA GPU with a virtual screen). `wayland` starts
a private headless sway (`WLR_BACKENDS=headless`, GPU rendering through the
render node with `WLR_RENDERER=gles2`, `--unsupported-gpu` on NVIDIA;
`WLR_RENDERER=pixman` on a machine without a GPU). sway rather than weston
because the Wayland probe needs the virtual-keyboard and screencopy protocols,
which weston does not offer. sway is not installed on the development box,
so the Wayland path is untested locally. The `xvfb` session has been run end to
end locally with all four terminals; on a bare Xvfb GNOME Terminal showed no
echo for XTest keys (the launcher now starts openbox when no window manager
is running; that fix is untested here because openbox is not installed).

### AWS runner

`scripts/bench-aws.sh` drives an instance that is already running and
reachable over SSH. It never launches or terminates instances.

```bash
# GPU instance (g4dn.xlarge, Ubuntu 26.04 when Ghostty is compared): X11 + Wayland
scripts/bench-aws.sh --host 203.0.113.10 --user ubuntu --key ~/.ssh/bench.pem --role gpu --ref main \
  --known-hosts /tmp/bench-known-hosts --compare-args "--terminals ultty,ghostty"

# Dedicated-core CPU instance (c7a.2xlarge or larger): headless A/A + instruction counts
scripts/bench-aws.sh --host 203.0.113.11 --user ubuntu --key ~/.ssh/bench.pem --role cpu --ref main
```

It sends the ref as a git bundle (the instance needs no repository access) and
runs `scripts/bench/aws-remote.sh` there. Setup installs build dependencies and
Rust (from `rust-toolchain.toml`), and for the GPU role `nvidia-driver-580`
(rebooting once and waiting for SSH), a headless Xorg configuration on the GPU,
sway, and the comparison terminals the run asks for (`--terminals` in
`--compare-args`; all four when absent):

- **Ghostty 1.4.0** built from the signed source release with Zig 0.17.0, each
  download checked against a pinned SHA-256 and the project's minisign key.
  Ghostty 1.4 needs GTK 4.18 and libadwaita 1.7, so use Ubuntu 26.04; the
  snap cannot drive the NVIDIA driver. `compare.sh` runs it on a private D-Bus
  session (it resets `GDK_DEBUG`, and GTK then waits on the desktop portal).
- **GNOME Terminal** from apt (version recorded; set `EXPECT_GNOME_TERMINAL` to
  a dpkg version to fail on drift).
- **Alacritty 0.16.1** built with `cargo install --locked`.

It then builds Ultty in release, runs the suite, and copies results back to
`target/bench-results/aws-<role>-<date>/` with a combined `REPORT.md`. Every
step is idempotent and any failure stops it with a non-zero exit. Pass
`--compare-args "--rounds 5 --k2p-samples 300"` to lengthen the campaign,
`--skip-setup` on reruns, and `--known-hosts FILE` to keep throwaway host
keys out of `~/.ssh/known_hosts`.

**Reusing a set-up instance.** Setup from a clean image takes about 25
minutes, most of it the Ghostty build and the driver install. After one
complete run, snapshot the instance (`aws ec2 create-image --instance-id ID
--name ultty-bench-gpu-<date> --no-reboot`) and launch later runs from that
image: setup then finds everything installed and only builds Ultty. The image
records driver and terminal versions as of its creation; make a new one when
the pins in `aws-remote.sh` change, and deregister old ones (with their EBS
snapshots) to stop storage charges.

**Safety stop.** Give every instance a time limit that survives the driver
reboot: a cloud-init `runcmd` with `systemd-run --on-active=5h systemctl
poweroff` covers the first boot only, so re-arm it (`sudo shutdown -h +240`)
after the reboot, and launch with `--instance-initiated-shutdown-behavior
terminate`. Do not put `shutdown` or `systemd-run` in `bootcmd`: on Ubuntu
26.04 the former powered the instance off at once and the latter hung boot.

## Required Sections

New measurement artifacts should include these sections:

1. **Environment**: date, commit, OS, CPU, relevant GPU/window state when known,
   terminal size for live runs, and any benchmark backend details.
2. **Commands**: exact commands run from the repository root.
3. **Summary**: a compact table with the headline numbers.
4. **Detail**: raw benchmark names, timing ranges, repeat counts, variance, or
   the exact workload shape.
5. **Interpretation**: what the result means for the current performance
   question, including whether the measurement supports a release claim, a
   regression gate, or only a local investigation.
6. **Caveats**: scheduler noise, window focus or occlusion, generated input
   overhead, missing GPU instrumentation, benchmark shortcuts, and anything
   else that could move the number.

## Naming

Use date-prefixed filenames for time-bound measurements:

```text
docs/perf/YYYY-MM-DD-<short-topic>.md
```

Keep durable design notes under descriptive names without dates when the file is
expected to evolve with the design.

## Release Claims

Only baseline artifacts should be used directly for README or release claims.
Live stress and investigation notes are useful for localising work and catching
regressions, but they include scheduler, PTY, compositor, and window-manager
noise. Promote a live number to a release claim only after documenting repeat
runs, variance, and the exact hardware/window conditions.

## Raw Output

Prefer preserving raw output inside the artifact when it is short. For longer
runs, store the generated log in `/tmp` during collection and summarise the
stable values in the artifact. Do not commit machine-local Criterion output,
flamegraphs, or temporary logs unless the file is intentionally curated.
