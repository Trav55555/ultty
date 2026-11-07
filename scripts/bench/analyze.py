#!/usr/bin/env python3
"""Summarise and compare benchmark results (JSON lines from ultty-bench and
the live drivers).

Run with `python3 -I`; it uses only the standard library.

    analyze.py results.jsonl                      # per-case summary
    analyze.py results.jsonl --a A --b B          # A/B comparison
    analyze.py aa.jsonl --aa                      # A/A: the session noise floor
    analyze.py ab.jsonl --mde-from aa.jsonl       # A/B gated by the measured floor
    analyze.py ... --markdown                     # tables for docs/perf
    analyze.py ... --json summary.json            # machine-readable summary
    analyze.py --self-test

Method (docs/perf/README.md has the reasoning):
- Summary: median, IQR, min, max, n per (suite, case, arm).
- A/B: ratio of medians B/A with a 95% bootstrap CI. Samples are paired by
  block when both arms share block numbers (the drivers interleave arms per
  block), so slow drift cancels; otherwise the arms are resampled
  independently. Mann-Whitney U (two-sided, normal approximation with tie
  correction) is reported as well; it is the right test for latency
  distributions.
- Verdict: with threshold t = max(--threshold, 2 x MDE), a change regresses
  only when the whole CI is past 1 - t in the bad direction, and improves only
  when the whole CI is past 1 + t in the good direction. Anything else is
  "no detectable change", which is not the same as "no change".
- A/A: run the same binary as both arms. The CI half-width is the minimum
  detectable effect (MDE) for that session; a CI that excludes 1.0 means the
  harness or the machine is not stable enough to trust.
"""

import argparse
import json
import math
import random
import sys
import unittest

HIGHER_IS_BETTER = {"MiB/s", "ops/s", "frames/s"}
# Fewer samples per arm than this get no verdict.
MIN_SAMPLES = 5


def quantile(sorted_values, q):
    """Linear interpolation between order statistics (numpy's default)."""
    if not sorted_values:
        raise ValueError("quantile of empty data")
    pos = q * (len(sorted_values) - 1)
    lo = math.floor(pos)
    hi = math.ceil(pos)
    return sorted_values[lo] + (sorted_values[hi] - sorted_values[lo]) * (pos - lo)


def summarize(values):
    s = sorted(values)
    return {
        "n": len(s),
        "min": s[0],
        "q1": quantile(s, 0.25),
        "median": quantile(s, 0.5),
        "q3": quantile(s, 0.75),
        "max": s[-1],
    }


def median(values):
    return quantile(sorted(values), 0.5)


def bootstrap_ratio(a_by_block, b_by_block, resamples, rng):
    """CI of median(B)/median(A). Inputs map block -> value."""
    common = sorted(set(a_by_block) & set(b_by_block))
    estimates = []
    if len(common) >= 2:
        pairs = [(a_by_block[k], b_by_block[k]) for k in common]
        n = len(pairs)
        for _ in range(resamples):
            pick = [pairs[rng.randrange(n)] for _ in range(n)]
            a_med = median([p[0] for p in pick])
            b_med = median([p[1] for p in pick])
            if a_med > 0:
                estimates.append(b_med / a_med)
        paired = True
    else:
        a = list(a_by_block.values())
        b = list(b_by_block.values())
        for _ in range(resamples):
            a_med = median([a[rng.randrange(len(a))] for _ in a])
            b_med = median([b[rng.randrange(len(b))] for _ in b])
            if a_med > 0:
                estimates.append(b_med / a_med)
        paired = False
    estimates.sort()
    return quantile(estimates, 0.025), quantile(estimates, 0.975), paired


def mann_whitney(a, b):
    """Two-sided Mann-Whitney U p-value (normal approximation, tie and
    continuity corrections). Returns (U for b, p)."""
    n1, n2 = len(a), len(b)
    combined = sorted([(v, 0) for v in a] + [(v, 1) for v in b])
    ranks = [0.0] * len(combined)
    tie_term = 0.0
    i = 0
    while i < len(combined):
        j = i
        while j + 1 < len(combined) and combined[j + 1][0] == combined[i][0]:
            j += 1
        avg = (i + j) / 2.0 + 1.0
        for k in range(i, j + 1):
            ranks[k] = avg
        t = j - i + 1
        tie_term += t ** 3 - t
        i = j + 1
    r2 = sum(r for r, (_, g) in zip(ranks, combined) if g == 1)
    u2 = r2 - n2 * (n2 + 1) / 2.0
    mu = n1 * n2 / 2.0
    n = n1 + n2
    var = n1 * n2 / 12.0 * ((n + 1) - tie_term / (n * (n - 1))) if n > 1 else 0.0
    if var <= 0:
        return u2, 1.0
    z = (abs(u2 - mu) - 0.5) / math.sqrt(var)
    p = math.erfc(max(z, 0.0) / math.sqrt(2.0))
    return u2, min(1.0, p)


def load(paths):
    rows = []
    for path in paths:
        with open(path, encoding="utf-8") as fh:
            for lineno, line in enumerate(fh, 1):
                line = line.strip()
                if not line:
                    continue
                try:
                    rows.append(json.loads(line))
                except json.JSONDecodeError as err:
                    raise SystemExit(f"{path}:{lineno}: not JSON: {err}") from err
    if not rows:
        raise SystemExit("no samples in input")
    return rows


def group(rows):
    """(suite, case) -> arm -> {block: value}, plus units and metadata."""
    groups = {}
    units = {}
    meta = {}
    for r in rows:
        key = (r["suite"], r["case"])
        arm = str(r.get("arm", "A"))
        blocks = groups.setdefault(key, {}).setdefault(arm, {})
        block = r.get("block", len(blocks))
        # Repeated block numbers (several samples per block) get unique keys.
        while block in blocks:
            block = f"{block}+"
        blocks[block] = float(r["value"])
        units[key] = r.get("unit", "")
        m = meta.setdefault(arm, {"revs": set(), "loads": [], "claims": set()})
        if r.get("rev"):
            m["revs"].add(str(r["rev"]) + ("+dirty" if r.get("dirty") else ""))
        if r.get("claim"):
            m["claims"].add(r["claim"])
        env = r.get("env") or {}
        for k in ("load_start", "load_end"):
            if isinstance(env.get(k), (int, float)):
                m["loads"].append(env[k])
    return groups, units, meta


def verdict(lo, hi, threshold, higher_better):
    if higher_better:
        if hi < 1 - threshold:
            return "regressed"
        if lo > 1 + threshold:
            return "improved"
    else:
        if lo > 1 + threshold:
            return "regressed"
        if hi < 1 - threshold:
            return "improved"
    return "no detectable change"


def compare(groups, units, a, b, args, mde):
    rng = random.Random(args.seed)
    results = []
    for key in sorted(groups):
        arms = groups[key]
        if a not in arms or b not in arms:
            continue
        av, bv = arms[a], arms[b]
        ratio = median(list(bv.values())) / median(list(av.values()))
        lo, hi, paired = bootstrap_ratio(av, bv, args.resamples, rng)
        _, p = mann_whitney(list(av.values()), list(bv.values()))
        unit = units[key]
        higher = unit in HIGHER_IS_BETTER
        floor = mde.get(key)
        threshold = max(args.threshold, 2 * floor) if floor is not None else args.threshold
        entry = {
            "suite": key[0],
            "case": key[1],
            "unit": unit,
            "a": summarize(list(av.values())),
            "b": summarize(list(bv.values())),
            "ratio": ratio,
            "ci": [lo, hi],
            "paired": paired,
            "mann_whitney_p": p,
            "threshold": threshold,
            "mde": floor,
        }
        n = min(len(av), len(bv))
        if n < MIN_SAMPLES:
            # A bootstrap of one or two values has a CI of zero width, which
            # would turn any difference into a confident verdict.
            entry["verdict"] = f"too few samples (n={n})"
        elif args.aa:
            entry["mde_half_width"] = (hi - lo) / 2
            entry["contains_one"] = lo <= 1.0 <= hi
            entry["verdict"] = "stable" if entry["contains_one"] else "UNSTABLE"
        else:
            entry["verdict"] = verdict(lo, hi, threshold, higher)
            if getattr(args, "neutral", False):
                # Comparing two products, not a change: say which is better.
                entry["verdict"] = {"regressed": f"{b} worse", "improved": f"{b} better"}.get(
                    entry["verdict"], entry["verdict"])
        results.append(entry)
    return results


def mde_from(path, args):
    rows = load([path])
    groups, units, _ = group(rows)
    arms = sorted({arm for g in groups.values() for arm in g})
    if len(arms) < 2:
        raise SystemExit(f"{path}: an A/A file needs two arms, found {arms}")
    ns = argparse.Namespace(**vars(args))
    ns.aa = True
    return {(r["suite"], r["case"]): r["mde_half_width"]
            for r in compare(groups, units, arms[0], arms[1], ns, {}) if "mde_half_width" in r}


def fmt(v):
    if v is None:
        return "-"
    if abs(v) >= 100:
        return f"{v:.0f}"
    if abs(v) >= 10:
        return f"{v:.1f}"
    return f"{v:.3g}"


def print_summary(groups, units, meta, markdown, out):
    if markdown:
        out.write("| Suite | Case | Arm | Median | IQR | Min | Max | n | Unit |\n")
        out.write("|---|---|---|---:|---:|---:|---:|---:|---|\n")
    for key in sorted(groups):
        for arm, blocks in sorted(groups[key].items()):
            s = summarize(list(blocks.values()))
            cells = [key[0], key[1], arm, fmt(s["median"]), fmt(s["q3"] - s["q1"]),
                     fmt(s["min"]), fmt(s["max"]), str(s["n"]), units[key]]
            if markdown:
                out.write("| " + " | ".join(f"`{c}`" if i == 1 else c
                                           for i, c in enumerate(cells)) + " |\n")
            else:
                out.write("  ".join(f"{c:<10}" for c in cells).rstrip() + "\n")
    print_meta(meta, markdown, out)


def print_meta(meta, markdown, out):
    out.write("\n")
    for arm, m in sorted(meta.items()):
        loads = m["loads"]
        load = f"{min(loads):.1f}-{max(loads):.1f}" if loads else "unknown"
        line = (f"arm {arm}: rev {', '.join(sorted(m['revs'])) or 'unknown'}; "
                f"claim {', '.join(sorted(m['claims'])) or 'unlabelled'}; load {load}")
        out.write(("- " if markdown else "") + line + "\n")
        if loads and max(loads) > 12:
            out.write(("- " if markdown else "") +
                      f"WARNING: arm {arm} ran with load above 12; treat it as noisy.\n")


def print_comparison(results, a, b, aa, markdown, out):
    if markdown:
        head = ["Suite", "Case", f"{a} median", f"{b} median", f"Ratio {b}/{a}",
                "95% CI", "MW p", "Verdict"]
        if aa:
            head[-1] = "MDE (±)"
        out.write("| " + " | ".join(head) + " |\n")
        out.write("|---|---|---:|---:|---:|---|---:|---|\n")
    for r in results:
        if aa and "mde_half_width" in r:
            last = f"{r['mde_half_width'] * 100:.1f}% {r['verdict']}"
        else:
            last = r["verdict"]
        cells = [r["suite"], r["case"], fmt(r["a"]["median"]), fmt(r["b"]["median"]),
                 f"{r['ratio']:.3f}", f"[{r['ci'][0]:.3f}, {r['ci'][1]:.3f}]",
                 f"{r['mann_whitney_p']:.3g}", last]
        if markdown:
            out.write("| " + " | ".join(f"`{c}`" if i == 1 else c
                                       for i, c in enumerate(cells)) + " |\n")
        else:
            out.write("  ".join(f"{c:<12}" for c in cells).rstrip() + "\n")


def main(argv):
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("files", nargs="*")
    ap.add_argument("--a", help="baseline arm label")
    ap.add_argument("--b", help="candidate arm label")
    ap.add_argument("--aa", action="store_true", help="treat the two arms as A/A")
    ap.add_argument("--neutral", action="store_true",
                    help="comparing products: verdicts say which arm is better, not \"regressed\"")
    ap.add_argument("--mde-from", help="A/A results file giving the per-case noise floor")
    ap.add_argument("--threshold", type=float, default=0.05)
    ap.add_argument("--resamples", type=int, default=10000)
    ap.add_argument("--seed", type=int, default=1)
    ap.add_argument("--markdown", action="store_true")
    ap.add_argument("--json", help="write the summary here")
    ap.add_argument("--fail-on-regression", action="store_true",
                    help="exit 3 when any case regressed (or an A/A is unstable)")
    ap.add_argument("--self-test", action="store_true")
    args = ap.parse_args(argv)

    if args.self_test:
        suite = unittest.TestLoader().loadTestsFromTestCase(SelfTest)
        ok = unittest.TextTestRunner(verbosity=1).run(suite).wasSuccessful()
        return 0 if ok else 1
    if not args.files:
        ap.error("no input files")

    rows = load(args.files)
    groups, units, meta = group(rows)
    arms = sorted({arm for g in groups.values() for arm in g})
    out = sys.stdout
    summary = {"arms": arms}

    if args.a or args.b or args.aa or len(arms) == 2:
        if len(arms) < 2:
            raise SystemExit(f"comparison needs two arms; found {arms}")
        a = args.a or arms[0]
        b = args.b or next(x for x in arms if x != a)
        mde = mde_from(args.mde_from, args) if args.mde_from else {}
        results = compare(groups, units, a, b, args, mde)
        if not results:
            raise SystemExit(f"no case has samples for both {a!r} and {b!r}")
        print_comparison(results, a, b, args.aa, args.markdown, out)
        print_meta(meta, args.markdown, out)
        summary["comparison"] = results
        bad = [r for r in results if r["verdict"] in ("regressed", "UNSTABLE")]
    else:
        print_summary(groups, units, meta, args.markdown, out)
        bad = []
    summary["cases"] = [
        {"suite": k[0], "case": k[1], "arm": arm, "unit": units[k],
         **summarize(list(v.values()))}
        for k in sorted(groups) for arm, v in sorted(groups[k].items())
    ]
    if args.json:
        with open(args.json, "w", encoding="utf-8") as fh:
            json.dump(summary, fh, indent=2, sort_keys=True)
            fh.write("\n")
    if args.fail_on_regression and bad:
        out.write(f"\n{len(bad)} case(s) regressed or unstable\n")
        return 3
    return 0


class SelfTest(unittest.TestCase):
    def ns(self, **kw):
        base = dict(threshold=0.05, resamples=2000, seed=7, aa=False)
        base.update(kw)
        return argparse.Namespace(**base)

    @staticmethod
    def arms(scale_b, n=12, seed=3, noise=0.04):
        rng = random.Random(seed)
        rows = []
        for block in range(n):
            drift = 1 + 0.1 * math.sin(block)  # shared by both arms in a block
            for arm, scale in (("A", 1.0), ("B", scale_b)):
                v = 200 * drift * scale * (1 + rng.uniform(-noise, noise))
                rows.append({"suite": "tp.vt", "case": "x", "arm": arm,
                             "block": block, "value": v, "unit": "MiB/s"})
        return rows

    def run_compare(self, rows, **kw):
        groups, units, _ = group(rows)
        return compare(groups, units, "A", "B", self.ns(**kw), {})[0]

    def test_quantiles_match_numpy(self):
        self.assertEqual(quantile([1, 2, 3, 4, 5, 6, 7, 8], 0.25), 2.75)
        self.assertEqual(quantile([1, 2, 3], 0.5), 2)

    def test_aa_contains_one(self):
        r = self.run_compare(self.arms(1.0, noise=0.02), aa=True)
        self.assertTrue(r["contains_one"], r["ci"])
        self.assertLess(r["mde_half_width"], 0.05)

    def test_twenty_percent_drop_is_a_regression(self):
        r = self.run_compare(self.arms(0.8))
        self.assertEqual(r["verdict"], "regressed")
        self.assertLess(r["ci"][1], 0.95)

    def test_twenty_percent_gain_is_an_improvement(self):
        self.assertEqual(self.run_compare(self.arms(1.2))["verdict"], "improved")

    def test_two_percent_is_not_called(self):
        self.assertEqual(self.run_compare(self.arms(0.98))["verdict"], "no detectable change")

    def test_pairing_cancels_shared_drift(self):
        r = self.run_compare(self.arms(0.9, noise=0.01))
        self.assertTrue(r["paired"])
        # Drift is +-10% but shared within a block, so the CI stays tight.
        self.assertLess(r["ci"][1] - r["ci"][0], 0.03)

    def test_latency_direction(self):
        rows = [dict(r, unit="us") for r in self.arms(1.3)]
        self.assertEqual(self.run_compare(rows)["verdict"], "regressed")

    def test_mde_raises_threshold(self):
        groups, units, _ = group(self.arms(0.92, noise=0.01))
        key = ("tp.vt", "x")
        strict = compare(groups, units, "A", "B", self.ns(), {})[0]
        loose = compare(groups, units, "A", "B", self.ns(), {key: 0.06})[0]
        self.assertEqual(strict["verdict"], "regressed")
        self.assertEqual(loose["verdict"], "no detectable change")
        self.assertAlmostEqual(loose["threshold"], 0.12)

    def test_too_few_samples_get_no_verdict(self):
        r = self.run_compare(self.arms(0.5, n=2))
        self.assertEqual(r["verdict"], "too few samples (n=2)")

    def test_neutral_wording_names_the_arm(self):
        r = self.run_compare(self.arms(0.8), neutral=True)
        self.assertEqual(r["verdict"], "B worse")

    def test_mann_whitney_separated_and_identical(self):
        _, p = mann_whitney(list(range(20)), list(range(100, 120)))
        self.assertLess(p, 1e-6)
        _, p = mann_whitney([1.0] * 10, [1.0] * 10)
        self.assertEqual(p, 1.0)

    def test_mann_whitney_matches_reference(self):
        # scipy.stats.mannwhitneyu([1,2,3,4,5],[3,4,5,6,7,8],
        #   method="asymptotic", use_continuity=True). By hand: U = 25.5,
        # mu = 15, tie-corrected var = 29.59, z = (10.5 - 0.5) / 5.44 = 1.838,
        # p = 2 * (1 - Phi(1.838)) = 0.0660.
        _, p = mann_whitney([1, 2, 3, 4, 5], [3, 4, 5, 6, 7, 8])
        self.assertAlmostEqual(p, 0.0660, places=3)


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
