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
# server1 is shared with other projects: wait up to 10 min for a 1-minute load under 2,
# then run anyway and flag the record (`noisy`), as the orchestrator asked.
LOAD_MAX, WAIT_MAX = 2.0, 600
w0 = time.monotonic()
while os.getloadavg()[0] >= LOAD_MAX and time.monotonic() - w0 < WAIT_MAX:
    time.sleep(15)
waited = round(time.monotonic() - w0)
uptime = subprocess.run(["uptime"], capture_output=True, text=True).stdout.strip()
load0 = os.getloadavg()[0]
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
    "noisy": load0 >= LOAD_MAX,
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
