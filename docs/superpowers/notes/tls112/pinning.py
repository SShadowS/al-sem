#!/usr/bin/env python3
"""TLS-112: does pinning, or running both sides at once, steady cdo-speed's
per-pass CPU and wall ratios? One binary against itself (master against master),
so every ratio should read 1.00. Uses cdo-speed's own Server and perf attach.

Arms (each run = one cdo-speed warm-up PASS PAIR: pass 1 and pass 2 with the
side order flipped, fresh servers per pass, the calls of cs.WARM_UP: the model
build plus warm prove/explain/digest/query/tests/impact; the rounds 1-3 are not run):
  unpinned        today's design: sides in turn, no affinity (all 22 threads).
                  Exactly cdo-speed's pass: first side builds timed, closed; second
                  side builds timed; first side again (untimed build); then the warm
                  calls, side order flipped per call and per pass.
  pinned          the same, both servers pinned to all 12 P threads (0-11).
  both            both sides' servers at once, each on 3 whole P-cores (both SMT
                  threads); pass 2 swaps the sets. Both builds and each warm call
                  are started together: each side's perf attaches first, then a
                  barrier, then both requests go out (skew logged per call).

Loads (burners run through the whole run, killed in a finally):
  alone           nothing of ours.
  e-burn          a busy loop on every E/LP-E thread (12-21): power and clock, no shared core.
  sib-burn        a busy loop on the second SMT thread of every P-core (2,4,5,7,9,11,
                  from the topology): another tenant on half the P threads. Same set
                  for every arm: unpinned and pinned servers share those 6 threads
                  (time-sliced) and their siblings' cores; in `both`, each side has
                  the tenant on one thread of each of its 3 cores, so both sides are
                  hit alike (a one-sided tenant is NOT tested).

Env: ROUNDS (3), ARMS, LOADS (comma lists), SEED (default: time), EXE, OUT.
Rows: OUT/rows.jsonl, one JSON line per pass, written as they finish.
`pinning.py --summary [rows.jsonl]` prints the spread per arm x load.
Run inside a CDO slot hold (timed for the real experiment).
"""
import importlib.machinery
import json
import os
import random
import signal
import statistics
import subprocess
import sys
import threading
import time

REPO = os.environ.get("REPO", "/work/alsem-wt/lane-tools-112m")
NOTES = "/work/.al-sem/tools-notes/tls112"
WS = "/work/src/cdo-baseline/Cloud"
OUT = os.environ.get("OUT", f"{NOTES}/run")
ROWS = f"{OUT}/rows.jsonl"
ROUNDS = int(os.environ.get("ROUNDS", "3"))
ARMS = os.environ.get("ARMS", "unpinned,pinned,both").split(",")
LOADS = os.environ.get("LOADS", "alone,e-burn,sib-burn").split(",")
SETTLE_S = 20
MIN_AVAIL_GB = 35
TEMP = "/sys/class/thermal/thermal_zone6/temp"  # x86_pkg_temp
EVENTS = [f"{pmu}/{e}" for pmu in ("cpu_core", "cpu_atom")
          for e in ("instructions/u", "cycles/u", "ref-cycles/u", "cycles/k")]


def topology():
    """(all threads, P threads, sibling pairs of the P-cores sorted by first thread)."""
    pairs = set()
    for c in range(os.cpu_count()):
        with open(f"/sys/devices/system/cpu/cpu{c}/topology/thread_siblings_list") as f:
            sib = []
            for part in f.read().strip().split(","):
                a, _, b = part.partition("-")
                sib += range(int(a), int(b or a) + 1)
        if len(sib) == 2:
            pairs.add(tuple(sorted(sib)))
    pairs = sorted(pairs)
    return set(range(os.cpu_count())), {c for p in pairs for c in p}, pairs


ALL, P_ALL, PAIRS = topology()
assert len(PAIRS) == 6, PAIRS
SET1 = {c for p in PAIRS[:3] for c in p}
SET2 = {c for p in PAIRS[3:] for c in p}
BURN = {"alone": [], "e-burn": sorted(ALL - P_ALL), "sib-burn": sorted(p[1] for p in PAIRS)}


def load_cdo_speed():
    cs = importlib.machinery.SourceFileLoader("cdo_speed", f"{REPO}/scripts/cdo-speed").load_module()
    events, why = cs.instr_probe()
    if not events:
        raise SystemExit(f"pinning: perf cannot count: {why}")
    # Our 8 events (4 per PMU, no multiplexing: 3 fixed counters + 1 general).
    cs.INSTR_EVENTS = tuple(EVENTS)
    cs.perf_event_args = lambda events=None: [a for e in EVENTS for a in ("-e", e)]
    return cs


cs = None


def read_first(path, conv=float):
    try:
        with open(path) as f:
            return conv(f.read().split()[0])
    except (OSError, ValueError, IndexError):
        return None


def temp():
    t = read_first(TEMP)
    return t and t / 1000


def mem_avail_gb():
    with open("/proc/meminfo") as f:
        for line in f:
            if line.startswith("MemAvailable:"):
                return int(line.split()[1]) / 1048576
    return 0.0


def suspended():
    r = subprocess.run([f"{REPO}/scripts/host-suspended"], capture_output=True, text=True)
    return int(r.stdout) if r.returncode == 0 and r.stdout.strip().isdigit() else None


def env_snapshot():
    return dict(temp=temp(), host_avg60=cs.psi_some([cs.HOST_PRESSURE], "avg60")[0],
                suspended=suspended(), mem_avail_gb=round(mem_avail_gb(), 1),
                t=time.time())


def wait_memory():
    """Refuse a run below MIN_AVAIL_GB (wait up to 10 min for it first)."""
    deadline = time.monotonic() + 600
    while mem_avail_gb() < MIN_AVAIL_GB:
        if time.monotonic() > deadline:
            raise SystemExit(f"pinning: MemAvailable {mem_avail_gb():.1f} GB < {MIN_AVAIL_GB} GB "
                             "for 10 min: refused")
        time.sleep(15)


def spawn(name, cpus):
    """A cs.Server whose whole tree (mcp, engine child) is pinned to `cpus`
    (inherited from this process at spawn), or unpinned for cpus=None."""
    old = os.sched_getaffinity(0)
    os.sched_setaffinity(0, cpus or ALL)
    try:
        return cs.Server(EXE, WS, OUT, name)
    finally:
        os.sched_setaffinity(0, old)


def tree_affinity(server):
    lists = set()
    for p in [server.p.pid, *cs.descendants(server.p.pid)]:
        try:
            with open(f"/proc/{p}/status") as f:
                lists |= {ln.split()[1] for ln in f if ln.startswith("Cpus_allowed_list:")}
        except OSError:
            pass
    return sorted(lists)


def measure(server, tool, args, barrier=None):
    """cs.Server.call, with the perf attach BEFORE the (optional) barrier, so
    two sides' requests leave together. Returns the call's record."""
    perf = server.instr_start()
    try:
        if barrier is not None:
            barrier.wait(timeout=120)
        u0, s0 = server.user_sys()
        t0 = time.monotonic()
        r = server.request("tools/call", {"name": tool, "arguments": args})
        wall = time.monotonic() - t0
        u1, s1 = server.user_sys()
    finally:
        parsed = server.instr_stop(perf)
    server.sample_hwm()
    if "error" in r:
        raise SystemExit(f"pinning: {server.name}: {tool}: {r['error']}")
    env = r["result"].get("structuredContent", {})
    if "stdoutFile" in env:
        sha = env["stdoutFile"].get("sha256", "")[:16]
    else:
        import hashlib
        text = env.get("stdoutText", env.get("output", ""))
        if not isinstance(text, str):
            text = json.dumps(text, sort_keys=True)
        sha = hashlib.sha256(text.encode()).hexdigest()[:16]
    user, system = u1 - u0, s1 - s0
    return dict(start=t0, wall=round(wall, 3), user=round(user, 2), sys=round(system, 2),
                cpu=round(user + system, 2), sha=sha, exit=env.get("exitCode", -1),
                counts=parsed and parsed[0],
                running_s=parsed and round(sum(parsed[1][e] for e in EVENTS
                                               if "instructions" in e) / 1e9, 3))


def measure_both(servers, tool, args):
    """Both sides' call at once (one thread each, a barrier after the attaches)."""
    barrier = threading.Barrier(2)
    out, errs = {}, {}

    def go(side):
        try:
            out[side] = measure(servers[side], tool, args, barrier)
        except BaseException as e:  # noqa: BLE001 — re-raised below
            errs[side] = e
            barrier.abort()

    ts = [threading.Thread(target=go, args=(s,)) for s in ("base", "head")]
    for t in ts:
        t.start()
    for t in ts:
        t.join()
    if errs:
        raise next(iter(errs.values()))
    return out


def pass_in_turn(p, cpus, tag):
    """cdo-speed's warm-up pass (main(): TLS-79 build order, run_round(0))."""
    calls = {}
    first, second = ("base", "head") if p % 2 == 0 else ("head", "base")
    servers = {}
    peaks = {}
    try:
        _, b_tool, b_args = cs.WARM_UP[0]
        servers[first] = spawn(f"{tag}-{first}", cpus)
        calls.setdefault("build", {})[first] = measure(servers[first], b_tool, b_args)
        aff = {first: tree_affinity(servers[first])}
        peaks[first] = servers[first].peak_mib()
        servers.pop(first).close()
        servers[second] = spawn(f"{tag}-{second}", cpus)
        calls["build"][second] = measure(servers[second], b_tool, b_args)
        aff[second] = tree_affinity(servers[second])
        servers[first] = spawn(f"{tag}-{first}-again", cpus)
        again = measure(servers[first], b_tool, b_args)  # untimed in cdo-speed
        order = ["base", "head"]
        for i, (cid, tool, args) in enumerate(cs.WARM_UP):
            if i == 0:
                continue
            side_order = order if i % 2 == 0 else order[::-1]
            if p % 2 == 1:
                side_order = side_order[::-1]
            for side in side_order:
                calls.setdefault(cid, {})[side] = measure(servers[side], tool, args)
        for side, s in servers.items():
            peaks[side] = max(peaks.get(side, 0), s.peak_mib())
    finally:
        for s in servers.values():
            s.close()
    return calls, dict(affinity=aff, again_build=again, peak_mib=peaks)


def pass_both(p, tag):
    sets = {"base": SET1, "head": SET2} if p % 2 == 0 else {"base": SET2, "head": SET1}
    servers = {}
    calls = {}
    try:
        for side in ("base", "head"):
            servers[side] = spawn(f"{tag}-{side}", sets[side])
        for cid, tool, args in cs.WARM_UP:
            calls[cid] = measure_both(servers, tool, args)
        aff = {side: tree_affinity(s) for side, s in servers.items()}
        peaks = {side: s.peak_mib() for side, s in servers.items()}
    finally:
        for s in servers.values():
            s.close()
    return calls, dict(affinity=aff, sets={k: sorted(v) for k, v in sets.items()},
                       peak_mib=peaks)


def ratios(calls):
    out = {}
    for cid, sides in calls.items():
        a, b = sides.get("base"), sides.get("head")
        if a and b:
            out[cid] = dict(cpu=b["cpu"] / a["cpu"] if a["cpu"] else None,
                            wall=b["wall"] / a["wall"] if a["wall"] else None,
                            same=a["sha"] == b["sha"], skew=round(abs(b["start"] - a["start"]), 3))
    return out


def run(rnd, arm, load, seed, out):
    wait_memory()
    burners = [subprocess.Popen(["taskset", "-c", str(c), "bash", "-c", "while :; do :; done"])
               for c in BURN[load]]
    try:
        for p in range(2):
            tag = f"r{rnd}-{arm}-{load}-p{p + 1}"
            before = env_snapshot()
            stalls = cs.Stalls()
            if arm == "both":
                calls, extra = pass_both(p, tag)
            else:
                calls, extra = pass_in_turn(p, P_ALL if arm == "pinned" else None, tag)
            row = dict(round=rnd, arm=arm, load=load, passno=p + 1, seed=seed,
                       burners=BURN[load], own_stall_pct=stalls.share(),
                       before=before, after=env_snapshot(), calls=calls,
                       ratio=ratios(calls), **extra)
            s0, s1 = before["suspended"], row["after"]["suspended"]
            row["void"] = None if s0 is None or s1 is None or s1 - s0 < 1 else f"suspended {s1 - s0} s"
            out.write(json.dumps(row) + "\n")
            out.flush()
            short = {c: f"cpu x{r['cpu']:.3f} wall x{r['wall']:.3f}{'' if r['same'] else ' SHA DIFFERS'}"
                     for c, r in row["ratio"].items() if c in ("build", "warm-tests")}
            print(f"{tag}: {short} void={row['void']}", flush=True)
    finally:
        for b in burners:
            b.kill()
            b.wait()


def main():
    global cs, EXE
    cs = load_cdo_speed()
    sha = subprocess.check_output(["git", "-C", REPO, "rev-parse", "--short=9", "HEAD"], text=True).strip()
    EXE = os.environ.get("EXE", f"{NOTES}/bin/alsem-{sha}")
    if not os.access(EXE, os.X_OK):
        raise SystemExit(f"pinning: no binary {EXE}")
    os.makedirs(OUT, exist_ok=True)
    seed = int(os.environ.get("SEED", int(time.time())))
    rng = random.Random(seed)
    print(f"pinning: exe {EXE} seed {seed} rounds {ROUNDS} arms {ARMS} loads {LOADS}\n"
          f"  P pairs {PAIRS} set1 {sorted(SET1)} set2 {sorted(SET2)} burn {BURN}", flush=True)
    signal.signal(signal.SIGTERM, lambda *_: sys.exit(143))  # so the finallys run
    t0 = time.monotonic()
    with open(ROWS, "a") as out:
        first = True
        for rnd in range(ROUNDS):
            combos = [(a, ld) for a in ARMS for ld in LOADS]
            rng.shuffle(combos)
            for arm, load in combos:
                if not first:
                    time.sleep(SETTLE_S)
                first = False
                t = time.monotonic()
                run(rnd, arm, load, seed, out)
                print(f"  run {rnd} {arm} {load}: {time.monotonic() - t:.0f} s", flush=True)
    print(f"pinning: done in {time.monotonic() - t0:.0f} s", flush=True)


def summary(path):
    rows = [json.loads(ln) for ln in open(path) if ln.strip()]
    print(f"{len(rows)} passes, {sum(1 for r in rows if r['void'])} void (excluded); "
          f"sha mismatches: {sum(1 for r in rows for x in r['ratio'].values() if not x['same'])}")
    rows = [r for r in rows if not r["void"]]
    calls = list(dict.fromkeys(c for r in rows for c in r["ratio"]))

    def spread(xs):
        xs = [x for x in xs if x]
        if not xs:
            return "-"
        return (f"n={len(xs):2d} min {min(xs):.3f} med {statistics.median(xs):.3f} "
                f"max {max(xs):.3f} max/min x{max(xs) / min(xs):.3f}")

    for arm in dict.fromkeys(r["arm"] for r in rows):
        for load in dict.fromkeys(r["load"] for r in rows if r["arm"] == arm):
            sel = [r for r in rows if r["arm"] == arm and r["load"] == load]
            print(f"\n{arm} / {load}")
            for c in calls:
                for m in ("cpu", "wall"):
                    per_pass = [r["ratio"][c][m] for r in sel if c in r["ratio"]]
                    # The pair's geometric mean, what cdo-speed judges.
                    pairs = {}
                    for r in sel:
                        if c in r["ratio"] and r["ratio"][c][m]:
                            pairs.setdefault(r["round"], []).append(r["ratio"][c][m])
                    gm = [statistics.geometric_mean(v) for v in pairs.values() if len(v) == 2]
                    print(f"  {c:<12} {m:<4} pass {spread(per_pass)}   pair-gm {spread(gm)}")
            skews = [x["skew"] for r in sel for x in r["ratio"].values()]
            skew = f"start skew max {max(skews):.3f} s; " if arm == "both" else ""
            print(f"  {skew}temp "
                  f"{min(r['before']['temp'] or 0 for r in sel)}-{max(r['after']['temp'] or 0 for r in sel)} C; "
                  f"own stall {[round(r['own_stall_pct'] or 0, 1) for r in sel]} %; "
                  f"peak MiB {max(sum(r['peak_mib'].values()) for r in sel):.0f}")


if __name__ == "__main__":
    if sys.argv[1:2] == ["--summary"]:
        summary(sys.argv[2] if len(sys.argv) > 2 else ROWS)
    else:
        main()
