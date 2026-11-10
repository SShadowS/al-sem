"""Timeline of a nextest JUnit run: running tests per second, the tail, and late long tests.

Usage: python3 timeline.py <junit.xml> [slots=22]
"""
import datetime as dt, re, sys

path = sys.argv[1]
slots = int(sys.argv[2]) if len(sys.argv) > 2 else 22
text = open(path).read()
t0 = dt.datetime.fromisoformat(re.search(r'<testsuites [^>]*timestamp="([^"]+)"', text).group(1))
wall = float(re.search(r'<testsuites [^>]*time="([\d.]+)"', text).group(1))
tests = []
for m in re.finditer(r'<testcase name="([^"]+)" classname="([^"]+)" timestamp="([^"]+)" time="([\d.]+)"', text):
    s = (dt.datetime.fromisoformat(m.group(3)) - t0).total_seconds()
    tests.append((s, s + float(m.group(4)), m.group(2), m.group(1)))
busy = [0.0] * (int(wall) + 2)
for s, e, _, _ in tests:
    for sec in range(int(s), int(e) + 1):
        lo, hi = max(s, sec), min(e, sec + 1)
        if hi > lo:
            busy[sec] += hi - lo
total = sum(e - s for s, e, _, _ in tests)
# The tail: from the last second the run had every slot busy.
full = [i for i, b in enumerate(busy) if b >= slots * 0.95]
tail_from = (full[-1] + 1) if full else 0
print(f"{path.rsplit('/', 1)[-1]}: wall {wall:.1f} s, summed {total:.0f} s, floor {total / slots:.1f} s, "
      f"last full-width second {tail_from} s, tail {wall - tail_from:.1f} s")
for a in range(0, int(wall) + 1, 10):
    seg = busy[a:a + 10]
    print(f"  {a:4d}-{a + 10:<4d} s  average running {sum(seg) / max(1, len(seg)):5.1f}")
late = sorted((t for t in tests if t[1] > tail_from), key=lambda t: -t[1])
print(f"\nrunning after {tail_from} s (start, end, binary, test):")
for s, e, b, n in late[:15]:
    print(f"  {s:6.1f} -> {e:6.1f}  ({e - s:5.1f} s)  {b}  {n}")
