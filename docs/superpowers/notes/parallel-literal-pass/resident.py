#!/usr/bin/env python3
"""TLS-22 plan task 5, resident part: `alsem mcp` on CDO at AL_SEM_REACH_WORKERS 1 and 8,
and --low-mem, each asked: a witness-case digest (20 roots, under the memo's 64), two such
digests at once, the 699-root diff impact (a streamed `trees` read), and two of those at
once. Records wall seconds and the summed peak RSS (VmHWM) of the server's processes after
each step. Reuses scripts/cdo-speed's client helpers.

Usage: resident.py <alsem> <out.jsonl>
"""
import importlib.machinery, json, os, subprocess, sys, tempfile, time

R = "/work/alsem-wt/lane-tools"
S = os.path.dirname(os.path.abspath(__file__))
speed = importlib.machinery.SourceFileLoader("cdo_speed", f"{R}/scripts/cdo-speed").load_module()
TOP = "/work/src/cdo-baseline"
WS = f"{TOP}/Cloud"
exe, out = sys.argv[1], sys.argv[2]
roots = open(f"{S}/roots40.txt").read().split("\n")[:40]
diff = open(f"{S}/../tls16/range.diff").read()


class Server(speed.Server):
    """cdo-speed's server, with extra `alsem mcp` flags and environment."""

    def __init__(self, name, extra, env):
        self.name = name
        self.root = tempfile.mkdtemp(prefix=f"tls22-{name}-", dir=S)
        self.err = open(f"{S}/{name}.server.err", "wb")
        self.p = subprocess.Popen(
            [exe, "mcp", "--workspace", WS, "--output-root", self.root, *extra],
            stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=self.err, env=env)
        self.buf, self.next_id, self.hwm_kib = b"", 0, {}
        r = self.request("initialize", {"protocolVersion": "2025-06-18", "capabilities": {},
                                        "clientInfo": {"name": "tls22", "version": "1"}})
        assert "result" in r, r
        self.send({"jsonrpc": "2.0", "method": "notifications/initialized", "params": {}})

    def together(self, calls):
        """Send every (tool, args) at once; wait for all. -> (wall s, [exit codes])."""
        t0 = time.monotonic()
        ids = []
        for tool, args in calls:
            self.next_id += 1
            ids.append(self.next_id)
            self.send({"jsonrpc": "2.0", "id": self.next_id, "method": "tools/call",
                       "params": {"name": tool, "arguments": args}})
        got = {}
        deadline = time.monotonic() + speed.CALL_TIMEOUT
        while len(got) < len(ids):
            msg = self.readline(deadline)
            if msg.get("id") in ids:
                got[msg["id"]] = msg
        wall = time.monotonic() - t0
        self.sample_hwm()
        codes = []
        for i in ids:
            m = got[i]
            if "error" in m:
                codes.append(f"error: {m['error']}")
            else:
                codes.append(m["result"].get("structuredContent", {}).get("exitCode", -1))
        return wall, codes


def digest(rs):
    return ("alsem_digest", {"routines": rs, "format": "json"})


IMPACT = ("alsem_impact", {"diff": diff, "diffRoot": TOP, "format": "json", "tests": False})
steps = [
    ("warm", [("alsem_explain", {"routines": [roots[0]], "format": "json"})]),
    ("witness", [digest(roots[0:20])]),
    ("witness x2", [digest(roots[20:30]), digest(roots[30:40])]),
    ("trees", [IMPACT]),
    ("trees x2", [IMPACT, IMPACT]),
]
for label, workers, extra in [("w1", "1", []), ("w8", "8", []), ("lowmem", "8", ["--low-mem"])]:
    env = speed.child_env()
    env["AL_SEM_REACH_WORKERS"] = workers
    srv = Server(f"res-{label}", extra, env)
    try:
        for step, calls in steps:
            wall, codes = srv.together(calls)
            rec = {"config": label, "step": step, "wall_s": round(wall, 1),
                   "peak_mib": round(srv.peak_mib(), 1), "exits": codes}
            print(json.dumps(rec), flush=True)
            with open(out, "a") as f:
                f.write(json.dumps(rec) + "\n")
    finally:
        srv.close()
