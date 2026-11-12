#!/usr/bin/env python3
"""TLS-22: why the --low-mem server's two concurrent 699-root impacts answered no exit code:
the same two asks once more, printing each answer's message."""
import importlib.machinery, json, os, subprocess, sys, tempfile, time

R = "/work/alsem-wt/lane-tools"
S = os.path.dirname(os.path.abspath(__file__))
speed = importlib.machinery.SourceFileLoader("cdo_speed", f"{R}/scripts/cdo-speed").load_module()
TOP = "/work/src/cdo-baseline"
exe = sys.argv[1]
diff = open(f"{S}/../tls16/range.diff").read()
root = tempfile.mkdtemp(prefix="tls22-lm-", dir=S)
p = subprocess.Popen([exe, "mcp", "--workspace", f"{TOP}/Cloud", "--output-root", root, "--low-mem"],
                     stdin=subprocess.PIPE, stdout=subprocess.PIPE, env=speed.child_env())
srv = speed.Server.__new__(speed.Server)
srv.name, srv.p, srv.buf, srv.next_id, srv.hwm_kib = "lm", p, b"", 0, {}
srv.request("initialize", {"protocolVersion": "2025-06-18", "capabilities": {},
                           "clientInfo": {"name": "tls22", "version": "1"}})
srv.send({"jsonrpc": "2.0", "method": "notifications/initialized", "params": {}})
args = {"diff": diff, "diffRoot": TOP, "format": "json", "tests": False}
for i in (1, 2):
    srv.send({"jsonrpc": "2.0", "id": 100 + i, "method": "tools/call",
              "params": {"name": "alsem_impact", "arguments": args}})
got = {}
deadline = time.monotonic() + 900
while len(got) < 2:
    m = srv.readline(deadline)
    if m.get("id") in (101, 102):
        got[m["id"]] = m
for i, m in sorted(got.items()):
    sc = m.get("result", {}).get("structuredContent", {})
    print(i, json.dumps({k: v for k, v in sc.items() if k not in ("stdoutText", "output")})[:800])
p.stdin.close()
p.wait(timeout=60)
