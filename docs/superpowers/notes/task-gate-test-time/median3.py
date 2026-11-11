"""Median per-test time over several nextest logs; top tests, binaries, modules, start order.

Usage: python3 median3.py <log>... (writes per-test medians to stdout as a ranked report)
"""
import collections, re, statistics, sys

pat = re.compile(r"^\s*(PASS|FAIL)\s+\[\s*([\d.]+)s\]\s+\(\s*(\d+)/\d+\)\s+(\S+)\s+(\S+)")
runs = []
walls = []
for log in sys.argv[1:]:
    d = {}
    for line in open(log, errors="replace"):
        m = pat.match(line)
        if m:
            d[(m.group(4), m.group(5))] = (float(m.group(2)), int(m.group(3)))
        s = re.search(r"Summary \[\s*([\d.]+)s\]", line)
        if s:
            walls.append(float(s.group(1)))
    runs.append(d)
keys = set.intersection(*(set(r) for r in runs))
med = {k: statistics.median(r[k][0] for r in runs) for k in keys}
# Finish order (the n in "( n/N)") of each test, median over runs.
fin = {k: statistics.median(r[k][1] for r in runs) for k in keys}
total = sum(med.values())
wall = statistics.median(walls)
print(f"runs {len(runs)}, walls {walls}, median wall {wall:.1f} s; tests {len(keys)}, "
      f"summed median test time {total:.0f} s, average parallelism {total / wall:.1f}")
top = sorted(med.items(), key=lambda kv: -kv[1])
acc = 0
print("\ntop 20 tests (median s, spread over runs, finished as n-th of N):")
for (b, n), t in top[:20]:
    acc += t
    ts = [r[(b, n)][0] for r in runs]
    print(f"  {t:6.1f} s  [{min(ts):5.1f}-{max(ts):5.1f}]  #{fin[(b, n)]:5.0f}  {b}  {n}")
print(f"  top 20: {acc:.0f} s = {100 * acc / total:.1f}% of summed test time")
for cut in (5, 10, 30):
    n = sum(1 for t in med.values() if t >= cut)
    s = sum(t for t in med.values() if t >= cut)
    print(f"  tests >= {cut:2d} s: {n:4d}, {s:6.0f} s = {100 * s / total:.1f}%")
by_bin = collections.defaultdict(lambda: [0.0, 0])
for (b, n), t in med.items():
    by_bin[b][0] += t
    by_bin[b][1] += 1
print("\nbinaries (summed median s, tests, share):")
for b, (s, c) in sorted(by_bin.items(), key=lambda kv: -kv[1][0]):
    print(f"  {s:7.0f} s  {c:5d}  {100 * s / total:5.1f}%  {b}")
by_mod = collections.Counter()
for (b, n), t in med.items():
    parts = n.split("::")
    by_mod[f"{b} {'::'.join(parts[:2]) if b == 'al-sem' else parts[0]}"] += t
print("\ntop 25 modules:")
for k, s in by_mod.most_common(25):
    print(f"  {s:7.0f} s  {100 * s / total:5.1f}%  {k}")
