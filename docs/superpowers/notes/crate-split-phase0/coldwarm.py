"""Phase 0, number 1: the cold share of build time, from agentflow gate logs.

Each cargo invocation in a log ends with `Finished <profile> ... in <T>`. The
`Compiling` lines since the previous `Finished` belong to it. An invocation is:
  cold-deps  : it compiled third-party crates (> DEP_MIN Compiling lines other than our own)
  warm-edit  : it compiled only our own crates (al-sem / al-syntax / benches / tools)
  no-op      : it compiled nothing
Usage: python3 coldwarm.py <log>...   (prints totals per class, per gate and per profile)
"""
import re, sys, collections

OWN = re.compile(r"Compiling (al-sem|al-syntax|al-sem-benches|census-probe)\b")
COMP = re.compile(r"^\s*Compiling (\S+)")
FIN = re.compile(r"^\s*Finished `([^`]+)` profile .* in (?:(\d+)m )?([\d.]+)s")
DEP_MIN = 5

tot = collections.defaultdict(lambda: [0, 0.0])
by_gate = collections.defaultdict(lambda: collections.defaultdict(lambda: [0, 0.0]))
by_prof = collections.defaultdict(lambda: collections.defaultdict(lambda: [0, 0.0]))
runs_cold = set()
for path in sys.argv[1:]:
    gate = path.split("/logs/")[1].split("/")[0]
    run = path.split("/runs/")[1].split("/")[0]
    deps = own = 0
    with open(path, errors="replace") as f:
        for line in f:
            m = COMP.match(line)
            if m:
                if OWN.search(line):
                    own += 1
                else:
                    deps += 1
                continue
            m = FIN.match(line)
            if m:
                secs = float(m.group(3)) + 60 * int(m.group(2) or 0)
                cls = "cold-deps" if deps > DEP_MIN else ("warm-edit" if own or deps else "no-op")
                if cls == "cold-deps":
                    runs_cold.add(run)
                for d in (tot[cls], by_gate[gate][cls], by_prof[m.group(1)][cls]):
                    d[0] += 1
                    d[1] += secs
                deps = own = 0

all_s = sum(v[1] for v in tot.values())
print(f"cargo build time in gate logs: {all_s/3600:.1f} h over {sum(v[0] for v in tot.values())} invocations")
for c, (n, s) in sorted(tot.items()):
    print(f"  {c:10s} n={n:5d}  {s/3600:6.2f} h  {100*s/all_s:5.1f}%")
for title, table in (("per gate", by_gate), ("per profile", by_prof)):
    print(title)
    for g, d in sorted(table.items(), key=lambda kv: -sum(v[1] for v in kv[1].values())):
        gs = sum(v[1] for v in d.values())
        parts = "  ".join(f"{c} {v[1]/3600:.2f}h/{v[0]}" for c, v in sorted(d.items()))
        print(f"  {g:24s} {gs/3600:6.2f} h  {parts}")
print("runs with a cold-deps build:", len(runs_cold))
