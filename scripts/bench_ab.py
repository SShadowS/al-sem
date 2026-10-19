#!/usr/bin/env python3
"""A/B bench of two alsem (or aldump) binaries in interleaved rounds.

Run:     scripts/bench_ab.py run --out DIR --a NAME=EXE --b NAME=EXE --rounds N
                             --cmd ID='ARGS' [--cmd ID='ARGS' ...]
Summary: scripts/bench_ab.py summary DIR/runs.jsonl

Each round runs every --cmd on both sides; the side order flips every round, so
neither side always runs first (warm cache, thermal state). Per run it records the
harness wall clock, the process peak working set (psutil, polled every 20 ms), the
trace's `peak.process_mb`/`peak.post_attach_mb`, and a hash of stdout (a JSON
report hashed without its `generatedAt`, so equal outputs hash equal). Every
`ALSEM_*` and `C1_CONE_CENSUS` variable is removed from the child environment; the
trace goes to DIR/<cmd>-<side>-<round>.trace.json, so
`scripts/trace_summary.py --compare 'DIR/d-A-*.trace.json' 'DIR/d-B-*.trace.json'`
attributes a wall difference to spans. No cargo build may run during a bench.

Example (#160): --a parent=H:/b/parent/alsem.exe --b head=H:/b/head/alsem.exe
  --cmd a='analyze U:/Git/DO-cdo-baseline/Cloud --format sarif'
  --cmd d='digest U:/Git/DO-cdo-baseline/Cloud --diff H:/b/c.diff'
"""
import argparse
import hashlib
import json
import os
import re
import shlex
import statistics
import sys
import time

PEAKS = re.compile(rb"\[perf-trace\] peak\.process_mb=(\d+) peak\.post_attach_mb=(\d+)")
METRICS = ("wall", "psutil_mb", "process_mb", "post_attach_mb")


def run_one(exe, args, out, name, side, cmd, rnd):
    import psutil

    env = {k: v for k, v in os.environ.items() if not k.startswith("ALSEM_") and k != "C1_CONE_CENSUS"}
    env.update(ALSEM_TRACE="1", ALSEM_TRACE_STDERR="1", ALSEM_TRACE_FILE=f"{out}/{name}.trace.json")
    t0 = time.perf_counter()
    with open(f"{out}/{name}.out", "wb") as so, open(f"{out}/{name}.err", "wb") as se:
        p = psutil.Popen([exe, *args], stdout=so, stderr=se, env=env)
        peak = 0
        while p.poll() is None:
            try:
                mi = p.memory_info()
                peak = max(peak, getattr(mi, "peak_wset", mi.rss))
            except psutil.Error:
                pass
            time.sleep(0.02)
    wall = time.perf_counter() - t0
    with open(f"{out}/{name}.err", "rb") as f:
        m = PEAKS.findall(f.read())
    with open(f"{out}/{name}.out", "rb") as f:
        body = f.read()
    try:  # a JSON report's `generatedAt` differs on every run
        doc = json.loads(body)
        if isinstance(doc, dict) and doc.pop("generatedAt", None) is not None:
            body = json.dumps(doc, sort_keys=True).encode()
    except ValueError:
        pass
    sha = hashlib.sha256(body).hexdigest()[:16]
    return dict(run=name, side=side, cmd=cmd, round=rnd, wall=wall, exit=p.returncode,
                psutil_mb=peak / 2**20, process_mb=int(m[-1][0]) if m else None,
                post_attach_mb=int(m[-1][1]) if m else None, out_sha=sha)


def run(args):
    sides = [s.split("=", 1) for s in (args.a, args.b)]
    cmds = [c.split("=", 1) for c in args.cmd]
    os.makedirs(args.out, exist_ok=True)
    log = f"{args.out}/runs.jsonl"
    for rnd in range(1, args.rounds + 1):
        order = sides if rnd % 2 else sides[::-1]
        for cmd, cmd_args in cmds:
            for side, exe in order:
                rec = run_one(exe, shlex.split(cmd_args, posix=True), args.out,
                              f"{cmd}-{side}-{rnd}", side, cmd, rnd)
                print(json.dumps(rec), flush=True)
                with open(log, "a") as f:
                    f.write(json.dumps(rec) + "\n")
    return summary(log, [s for s, _ in sides])


def summary(log, sides=None):
    with open(log) as f:
        rs = [json.loads(line) for line in f if line.strip()]
    sides = sides or list(dict.fromkeys(r["side"] for r in rs))
    if len(sides) != 2:
        print(f"need exactly two sides, got {sides}")
        return 2
    a, b = sides
    for r in rs:  # records written before `round` existed: the run name ends in it
        r.setdefault("round", int(re.findall(r"\d+", r["run"])[-1]))
    for cmd in dict.fromkeys(r.get("cmd", "") for r in rs):
        cr = [r for r in rs if r.get("cmd", "") == cmd]
        fails = [r["run"] for r in cr if r["exit"] != 0]
        print(f"\n=== {cmd or '(no cmd)'}: {a} vs {b}" + (f"   NON-ZERO EXIT: {fails}" if fails else ""))
        print(f"{'metric':<16}{a + ' med':>12}{'band':>9}{b + ' med':>12}{'band':>9}{'B - A':>10}  B higher")
        for key in METRICS:
            va = {r["round"]: r[key] for r in cr if r["side"] == a and r.get(key) is not None}
            vb = {r["round"]: r[key] for r in cr if r["side"] == b and r.get(key) is not None}
            if not va or not vb:
                continue
            pairs = sorted(set(va) & set(vb))
            higher = sum(vb[p] > va[p] for p in pairs)
            ma, mb = statistics.median(va.values()), statistics.median(vb.values())
            print(f"{key:<16}{ma:>12.1f}{max(va.values()) - min(va.values()):>9.1f}{mb:>12.1f}"
                  f"{max(vb.values()) - min(vb.values()):>9.1f}{mb - ma:>+10.1f}  {higher}/{len(pairs)}")
        for side in (a, b):
            shas = sorted({r["out_sha"] for r in cr if r["side"] == side and "out_sha" in r})
            print(f"stdout sha256 {side}: {shas}")
    return 0


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = ap.add_subparsers(dest="mode", required=True)
    r = sub.add_parser("run")
    r.add_argument("--out", required=True)
    r.add_argument("--a", required=True, help="NAME=EXE")
    r.add_argument("--b", required=True, help="NAME=EXE")
    r.add_argument("--rounds", type=int, required=True)
    r.add_argument("--cmd", action="append", required=True, help="ID='ARGS' (repeatable)")
    s = sub.add_parser("summary")
    s.add_argument("log")
    args = ap.parse_args()
    return run(args) if args.mode == "run" else summary(args.log)


if __name__ == "__main__":
    sys.exit(main())
