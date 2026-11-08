#!/usr/bin/env python3
"""Summarize a soak run's samples.csv (scripts/bench/soak.sh).

    scripts/bench/soak-report.py <samples.csv> [--warmup MINUTES]

Reports resident memory (RSS plus anything swapped out, when the CSV has
swap_kib; RSS alone falls when the host swaps Ultty out) at the end of
warm-up and at the end, its least-squares trend after warm-up in MiB per
hour, the peak, and the frame time median and p99
across the run (the per-minute p99s' median and worst). Exits 1 when RSS
grows by more than 1 MiB an hour after warm-up.
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


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("samples")
    parser.add_argument("--warmup", type=int, default=30, help="minutes to skip")
    args = parser.parse_args()
    with open(args.samples) as f:
        rows = [{k: float(v) for k, v in row.items()} for row in csv.DictReader(f)]
    if not rows:
        sys.exit("no samples")
    for r in rows:
        r["mem_kib"] = r["rss_kib"] + r.get("swap_kib", 0.0)
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
    label = "RSS + swap" if "swap_kib" in rows[0] else "RSS (swap not recorded)"
    print(f"{label}: {steady[0]['mem_kib'] / 1024:.1f} MiB after warm-up, "
          f"{rows[-1]['mem_kib'] / 1024:.1f} MiB at the end, "
          f"peak {max(r['mem_kib'] for r in rows) / 1024:.1f} MiB")
    if max(r.get("swap_kib", 0.0) for r in rows) > 0:
        print(f"swapped out at the end: {rows[-1]['swap_kib'] / 1024:.1f} MiB")
    print(f"memory trend after warm-up: {trend:+.2f} MiB/hour")
    print(f"GPU memory in host RAM: {rows[-1]['gpu_kib'] / 1024:.1f} MiB at the end")
    print(f"CPU: {rows[-1]['cpu_s'] - rows[0]['cpu_s']:.0f} s over the run")
    if p99s:
        print(f"frame time: median of per-minute medians {statistics.median(p50s):.2f} ms, "
              f"per-minute p99 median {statistics.median(p99s):.2f} ms, worst {max(p99s):.2f} ms")
    if trend > MAX_GROWTH_MIB_PER_HOUR:
        print(f"FAIL: memory grows more than {MAX_GROWTH_MIB_PER_HOUR} MiB/hour")
        sys.exit(1)


if __name__ == "__main__":
    main()
