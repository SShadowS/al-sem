#!/usr/bin/env python3
"""Summarize an alsem Chrome-trace: SELF time first, inclusive second.

SELF time (a span's duration minus its nested children on the same tid) is the
column that ranks work. Ranking by inclusive total hid 24.8% of an 8020 run --
18.9 s inside `analyze.total` and `l4_l5.run_detectors`, two long-lived brackets
whose children do not tile them -- for this track's entire history.

Usage: scripts/trace_summary.py <trace.json> [<trace.json> ...]
       scripts/trace_summary.py --compare '<A glob>' '<B glob>' [min_abs_diff_ms]

--compare puts two sets of runs side by side (quote the globs): per span, the
median inclusive and self ms of each side, the band (max - min), B - A, and in
how many pairs B was slower. Runs pair by the number their file name ends in
(`d-parent-3.trace.json` with `d-head-3.trace.json`), so an interleaved round
compares with itself. One bench's runs are too few to call a difference real
when it sits inside a side's band.
"""
import glob
import json
import re
import statistics
import sys
from collections import defaultdict


def agg(path):
    """Return (inclusive_ms, self_ms, count, peak_mb) keyed by span name."""
    with open(path, "r", encoding="utf-8") as f:
        events = json.load(f)
    stack = defaultdict(list)          # tid -> [name, start_ts, child_us]
    inclusive, selft, count = defaultdict(float), defaultdict(float), defaultdict(int)
    peak = 0
    for e in events:
        tid = e.get("tid", 0)
        if e.get("ph") == "B":
            stack[tid].append([e["name"], e["ts"], 0.0])
        elif e.get("ph") == "E":
            if not stack[tid]:
                continue           # unmatched E (truncated trace) -- ignore
            name, ts0, kids = stack[tid].pop()
            dur = e["ts"] - ts0
            inclusive[name] += dur / 1000.0
            selft[name] += (dur - kids) / 1000.0
            count[name] += 1
            if stack[tid]:
                stack[tid][-1][2] += dur
            pm = (e.get("args") or {}).get("peak_mb")
            if pm and pm > peak:
                peak = pm
    return inclusive, selft, count, peak


def by_round(pattern):
    """{round: path} for a glob; the round is the last number in the name."""
    runs = {}
    for path in glob.glob(pattern):
        nums = re.findall(r"\d+", path.replace("\\", "/").rsplit("/", 1)[-1])
        if nums:
            runs[int(nums[-1])] = path
    return runs


def compare(pattern_a, pattern_b, threshold):
    a, b = by_round(pattern_a), by_round(pattern_b)
    if not a or not b:
        print(f"no runs: A {len(a)}, B {len(b)}")
        return 2
    aggs = {side: {r: agg(p) for r, p in runs.items()} for side, runs in (("A", a), ("B", b))}
    pairs = sorted(set(a) & set(b))
    print(f"A {len(a)} runs: {pattern_a}\nB {len(b)} runs: {pattern_b}\npairs: {len(pairs)}")
    for col, ix in (("inclusive", 0), ("self", 1)):
        rows = []
        for name in set().union(*(x[ix] for s in aggs.values() for x in s.values())):
            va = [x[ix].get(name, 0.0) for x in aggs["A"].values()]
            vb = [x[ix].get(name, 0.0) for x in aggs["B"].values()]
            diff = statistics.median(vb) - statistics.median(va)
            slower = sum(aggs["B"][r][ix].get(name, 0.0) > aggs["A"][r][ix].get(name, 0.0) for r in pairs)
            rows.append((name, statistics.median(va), max(va) - min(va),
                         statistics.median(vb), max(vb) - min(vb), diff, slower))
        print(f"\n--- {col} ms, |B - A| >= {threshold:g} ---")
        print(f"{'span':<46}{'A med':>10}{'A band':>9}{'B med':>10}{'B band':>9}{'B - A':>9}  B slower")
        for name, ma, ba, mb, bb, diff, slower in sorted(rows, key=lambda r: -abs(r[5])):
            if abs(diff) >= threshold:
                print(f"{name:<46}{ma:>10.1f}{ba:>9.1f}{mb:>10.1f}{bb:>9.1f}{diff:>+9.1f}  {slower}/{len(pairs)}")
    return 0


def main():
    if len(sys.argv) < 2:
        print(__doc__)
        return 2
    if sys.argv[1] == "--compare":
        if len(sys.argv) not in (4, 5):
            print(__doc__)
            return 2
        return compare(sys.argv[2], sys.argv[3], float(sys.argv[4]) if len(sys.argv) == 5 else 200)
    for path in sys.argv[1:]:
        inclusive, selft, count, peak = agg(path)
        root = inclusive.get("analyze.total", 0.0) or 1.0
        print(f"\n=== {path}   analyze.total={root/1000:.1f}s   peak_mb={peak} ===")
        print(f"{'span':<46}{'self ms':>10}{'incl ms':>10}{'n':>6}{'self%':>8}")
        for name, ms in sorted(selft.items(), key=lambda kv: -kv[1]):
            if ms < 40:
                continue
            print(f"{name:<46}{ms:>10.1f}{inclusive[name]:>10.1f}"
                  f"{count[name]:>6}{100.0*ms/root:>7.1f}%")
    return 0


if __name__ == "__main__":
    sys.exit(main())
