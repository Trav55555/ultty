#!/usr/bin/env python3
"""Summarize a soak run's samples.csv (scripts/bench/soak.sh).

    scripts/bench/soak-report.py <samples.csv> [--warmup MINUTES]

Reports Ultty's own memory, anonymous RSS plus swap (when the CSV has
anon_kib and swap_kib), at the end of warm-up and at the end, its least-squares trend after warm-up in MiB per
hour, the peak, and the RSS total, and the frame time median and p99
across the run (the per-minute p99s' median and worst). With heap_kib
(ultty built with --features heap-stats), also the live Rust heap and its
trend, which excludes the GPU driver's and the allocator's memory. With draw times
and window sizes in the CSV, also the draw time per window size in the
first and last hour after warm-up. Exits 1 when memory
grows by more than 1 MiB an hour after warm-up.

Plain RSS is not judged: under memory pressure the kernel drops the
mapped libraries' pages (the GPU driver is tens of MiB) and swaps heap
out, so RSS can fall while a leak grows.
"""

import argparse
import csv
import statistics
import sys

MAX_GROWTH_MIB_PER_HOUR = 1.0


def slope(xs, ys):
    mx, my = statistics.fmean(xs), statistics.fmean(ys)
    den = sum((x - mx) ** 2 for x in xs)
    return sum((x - mx) * (y - my) for x, y in zip(xs, ys)) / den if den else 0.0


def report_drift(steady):
    """Draw time per window size, first hour after warm-up against the last.

    Draw time is Ultty's own work per frame (not presenting), so a rise at
    the same window size means per-frame cost grows with the session.
    """
    if len(steady) < 120:
        print("draw time drift: needs two hours after warm-up")
        return
    first = steady[:60]
    last = steady[-60:]
    print("draw time per window size (median of per-minute p99s), "
          "first hour after warm-up -> last hour:")
    for size in sorted({(r["width"], r["height"]) for r in steady}):
        def p99(rs):
            v = [r["draw_p99_us"] / 1000 for r in rs
                 if (r["width"], r["height"]) == size and r["frames"] > 0]
            return statistics.median(v) if v else None
        a, b = p99(first), p99(last)
        if a is None or b is None:
            continue
        print(f"  {size[0]:.0f}x{size[1]:.0f}: {a:.2f} -> {b:.2f} ms ({(b / a - 1) * 100:+.0f}%)")


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("samples")
    parser.add_argument("--warmup", type=int, default=30, help="minutes to skip")
    args = parser.parse_args()
    with open(args.samples) as f:
        rows = [{k: float(v) for k, v in row.items() if v != ""} for row in csv.DictReader(f)]
    if not rows:
        sys.exit("no samples")
    for r in rows:
        r["mem_kib"] = r.get("anon_kib", r["rss_kib"]) + r.get("swap_kib", 0.0)
    steady = [r for r in rows if r["minute"] > args.warmup]
    if len(steady) < 2:
        print(f"{len(rows)} minutes sampled: too short to judge growth after a "
              f"{args.warmup}-minute warm-up")
        sys.exit(0)
    minutes = [r["minute"] for r in steady]
    mem_mib = [r["mem_kib"] / 1024 for r in steady]
    trend = slope(minutes, mem_mib) * 60
    p99s = [r["frame_p99_us"] / 1000 for r in rows if r["frames"] > 0]
    p50s = [r["frame_p50_us"] / 1000 for r in rows if r["frames"] > 0]

    print(f"minutes sampled: {len(rows)} (warm-up {args.warmup})")
    label = ("anonymous RSS + swap" if "anon_kib" in rows[0]
             else "RSS (anonymous memory not recorded)")
    print(f"{label}: {steady[0]['mem_kib'] / 1024:.1f} MiB after warm-up, "
          f"{rows[-1]['mem_kib'] / 1024:.1f} MiB at the end, "
          f"peak {max(r['mem_kib'] for r in rows) / 1024:.1f} MiB")
    print(f"RSS: {rows[-1]['rss_kib'] / 1024:.1f} MiB at the end, "
          f"peak {max(r['rss_kib'] for r in rows) / 1024:.1f} MiB")
    if max(r.get("swap_kib", 0.0) for r in rows) > 0:
        print(f"swapped out at the end: {rows[-1]['swap_kib'] / 1024:.1f} MiB")
    print(f"memory trend after warm-up: {trend:+.2f} MiB/hour")
    heap = [r for r in steady if "heap_kib" in r]
    if len(heap) >= 2:
        heap_trend = slope([r["minute"] for r in heap],
                           [r["heap_kib"] / 1024 for r in heap]) * 60
        print(f"Rust heap (live, counted): {heap[0]['heap_kib'] / 1024:.1f} MiB after warm-up, "
              f"{heap[-1]['heap_kib'] / 1024:.1f} MiB at the end, "
              f"peak {max(r['heap_kib'] for r in heap) / 1024:.1f} MiB, "
              f"trend {heap_trend:+.2f} MiB/hour")
    print(f"GPU memory in host RAM: {rows[-1]['gpu_kib'] / 1024:.1f} MiB at the end")
    print(f"CPU: {rows[-1]['cpu_s'] - rows[0]['cpu_s']:.0f} s over the run")
    if p99s:
        print(f"frame time: median of per-minute medians {statistics.median(p50s):.2f} ms, "
              f"per-minute p99 median {statistics.median(p99s):.2f} ms, worst {max(p99s):.2f} ms")
    if "draw_p99_us" in rows[0]:
        report_drift(steady)
    if trend > MAX_GROWTH_MIB_PER_HOUR:
        print(f"FAIL: memory grows more than {MAX_GROWTH_MIB_PER_HOUR} MiB/hour")
        sys.exit(1)


if __name__ == "__main__":
    main()
