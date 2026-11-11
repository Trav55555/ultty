#!/usr/bin/env python3
"""Summarise a pty-flood-ab.sh run: per-arm medians, paired ratios, frame residue.

    pty-flood-analyze.py OUT_DIR

Arm A is the baseline, arm B the candidate. Batch 0 is the discarded trial
round. Throughput is 32 MiB over the median per-run time of a criterion run.
The ratio is candidate / baseline throughput per round, with a 95% percentile
bootstrap interval over rounds (10,000 resamples, fixed seed). The residue
table uses only samples of one iteration, whose time is a single flood, and
reports each run time modulo the 16 ms frame.
"""
import collections
import json
import pathlib
import random
import statistics
import sys

MIB = 32.0
FRAME_NS = 16_000_000


def quantiles(xs):
    xs = sorted(xs)
    q = statistics.quantiles(xs, n=4, method="inclusive") if len(xs) > 1 else [xs[0]] * 3
    return q[0], q[1], q[2]


def main(out):
    out = pathlib.Path(out)
    rows = [r.split("\t") for r in (out / "runs.tsv").read_text().splitlines()[1:]]
    mibs = collections.defaultdict(dict)  # (case,size,batch,round) -> arm -> MiB/s
    loads = []
    for batch, rnd, arm, case, size, load, ns in rows:
        if batch == "0":
            continue
        mibs[(case, size, int(batch), int(rnd))][arm] = MIB / (float(ns) / 1e9)
        loads.append(float(load))
    cases = sorted({k[:2] for k in mibs}, key=lambda k: (["ascii", "cjk", "sgr-dense"].index(k[0]), k[1]))
    rng = random.Random(1)

    print("| case | arm | median MiB/s | IQR / median | range |")
    print("|---|---|---:|---:|---:|")
    for case, size in cases:
        for arm, name in (("A", "A"), ("B", "B")):
            xs = [v[arm] for k, v in mibs.items() if k[:2] == (case, size)]
            q1, med, q3 = quantiles(xs)
            print(f"| {case} {size} | {name} | {med:.1f} | {100 * (q3 - q1) / med:.1f}% | {min(xs):.1f}-{max(xs):.1f} |")

    print("\n| case | B / A | 95% CI | IQR of ratio | batches 1, 2, 3 |")
    print("|---|---:|---|---|---|")
    for case, size in cases:
        keyed = {k[2:]: v["B"] / v["A"] for k, v in mibs.items() if k[:2] == (case, size)}
        ratios = list(keyed.values())
        boots = sorted(
            statistics.median(rng.choices(ratios, k=len(ratios))) for _ in range(10_000)
        )
        lo, hi = boots[249], boots[9749]
        q1, med, q3 = quantiles(ratios)
        per_batch = [
            statistics.median([r for (b, _), r in keyed.items() if b == batch])
            for batch in (1, 2, 3)
        ]
        print(
            f"| {case} {size} | {med:.3f} | {lo:.3f}-{hi:.3f} | {q1:.3f}-{q3:.3f} | "
            + ", ".join(f"{x:.3f}" for x in per_batch)
            + " |"
        )

    print(f"\n1-minute load while running: {min(loads):.2f} to {max(loads):.2f}")

    print("\n| case | arm | single-flood samples | residue mod 16 ms: min / median / max (ms) | distinct 1 ms bins of 16 |")
    print("|---|---|---:|---|---:|")
    for case, size in cases:
        for arm in "AB":
            res = []
            for f in out.glob(f"raw/[1-9]*-{arm}-{case}-{size}.json"):
                s = json.loads(f.read_text())
                res += [t % FRAME_NS for t, i in zip(s["times"], s["iters"]) if i == 1]
            if not res:
                print(f"| {case} {size} | {arm} | 0 | n/a | n/a |")
                continue
            bins = len({int(r // 1e6) for r in res})
            print(
                f"| {case} {size} | {arm} | {len(res)} | {min(res) / 1e6:.1f} / "
                f"{statistics.median(res) / 1e6:.1f} / {max(res) / 1e6:.1f} | {bins} |"
            )


if __name__ == "__main__":
    main(sys.argv[1])
