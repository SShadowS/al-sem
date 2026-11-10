"""Phase 0 timing harness: run one command, record wall time, peak child RSS and load.

Usage: python3 timeit.py <label> <out.jsonl> -- <command...>
Appends one JSON line: label, command, wall seconds, exit code, peak RSS of the largest
child process (ru_maxrss of RUSAGE_CHILDREN, so the largest single rustc/linker/test, not the
sum), user+sys CPU seconds of all children, the 1-minute load before and after, the number of
`Running `rustc` lines when the command was run with -v (0 otherwise), and the number of
`Compiling` lines. Output of the command goes to <out.jsonl>.<label>.log.
There is no /usr/bin/time in the container; this replaces it.
"""
import json, os, resource, subprocess, sys, time

label, out = sys.argv[1], sys.argv[2]
cmd = sys.argv[sys.argv.index("--") + 1:]
log = f"{out}.{label}.log"
# server1 is shared with other projects. The first T1 pair waited 10 minutes each for a
# 1-minute load under 2 and never got it: the host's background load sits at 3-5, and the
# load average trails our own previous build for minutes. So the guard now samples what a
# timing feels: the share of idle CPU over 5 seconds (/proc/stat, host-wide). It waits up to
# 3 minutes for >= 90% idle, then runs anyway and flags the record (`noisy`). The 1-minute
# load is still recorded.
IDLE_MIN, WAIT_MAX = 0.90, 180

def idle_share(secs=5):
    def snap():
        v = list(map(int, open("/proc/stat").readline().split()[1:]))
        return v[3] + v[4], sum(v)
    i0, t0_ = snap()
    time.sleep(secs)
    i1, t1_ = snap()
    return (i1 - i0) / max(1, t1_ - t0_)

w0 = time.monotonic()
idle = idle_share()
while idle < IDLE_MIN and time.monotonic() - w0 < WAIT_MAX:
    idle = idle_share()
waited = round(time.monotonic() - w0)
uptime = subprocess.run(["uptime"], capture_output=True, text=True).stdout.strip()
load0 = os.getloadavg()[0]
LOAD_MAX = None
t0 = time.monotonic()
with open(log, "w") as f:
    rc = subprocess.call(cmd, stdout=f, stderr=subprocess.STDOUT)
wall = time.monotonic() - t0
ru = resource.getrusage(resource.RUSAGE_CHILDREN)
text = open(log, errors="replace").read()
rec = {
    "label": label,
    "cmd": " ".join(cmd),
    "wall_s": round(wall, 2),
    "exit": rc,
    "peak_child_rss_mib": round(ru.ru_maxrss / 1024, 1),
    "cpu_s": round(ru.ru_utime + ru.ru_stime, 1),
    "load_before": round(load0, 2),
    "noisy": idle < IDLE_MIN,
    "idle_before": round(idle, 3),
    "waited_s": waited,
    "uptime_before": uptime,
    "load_after": round(os.getloadavg()[0], 2),
    "rustc_runs": text.count("Running `") ,
    "compiling": sum(1 for l in text.splitlines() if l.lstrip().startswith("Compiling ")),
    "at": time.strftime("%Y-%m-%dT%H:%M:%S"),
}
with open(out, "a") as f:
    f.write(json.dumps(rec) + "\n")
print(json.dumps(rec))
sys.exit(rc)
