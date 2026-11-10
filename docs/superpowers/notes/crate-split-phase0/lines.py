"""Phase 0 line counts: production vs test lines per planned group.

Test lines = whole files named *test*.rs, plus, in other files, everything from the
first `#[cfg(test)]` that is followed (within 2 lines) by `mod ` to the end of file
(the repo's inline-test convention). Rough, as section 2 says.
"""
import os, re, sys, collections

root = sys.argv[1]
GROUPS = [
    ("src/engine/l5/detectors", "l5 detectors"),
    ("src/engine/l5", "l5 (rest)"),
    ("src/program/resolve", "program/resolve"),
    ("src/engine/l4", "l4"),
    ("src/engine/resident", "engine upper"), ("src/engine/impact", "engine upper"),
    ("src/engine/test_select", "engine upper"), ("src/engine/deps", "engine upper"),
    ("src/engine/host", "engine upper"), ("src/engine/batch", "engine upper"),
    ("src/engine/gate", "gate"),
    ("src/program/model", "program/model"),
    ("src/program", "program other + flat"),
    ("src/snapshot", "snapshot"),
    ("src/lsp", "lsp/mcp/wire"), ("src/mcp", "lsp/mcp/wire"), ("src/wire", "lsp/mcp/wire"),
    ("src/bin", "bin"),
    ("src/engine", "engine flat files"),
    ("src", "src flat files + other"),
    ("crates/al-syntax", "al-syntax"),
    ("tests", "tests/"),
    ("benches", "benches"),
    ("tools", "tools"),
]

def group(rel):
    for prefix, name in GROUPS:
        if rel == prefix or rel.startswith(prefix + "/"):
            return name
    return None

def split(path):
    with open(path, encoding="utf-8", errors="replace") as f:
        lines = f.read().split("\n")
    n = len(lines) - (1 if lines and lines[-1] == "" else 0)
    base = os.path.basename(path)
    if base == "tests.rs" or base.endswith("_tests.rs") or "_tests_" in base or "/tests/" in path:
        return 0, n
    for i, l in enumerate(lines):
        if l.strip().startswith("#[cfg(test)]"):
            nxt = " ".join(x.strip() for x in lines[i + 1:i + 3])
            if re.match(r"(pub(\([^)]*\))? )?mod \w+", nxt):
                return i, n - i
    return n, 0

tot = collections.defaultdict(lambda: [0, 0])
for dp, dns, fns in os.walk(root):
    dns[:] = [d for d in dns if d not in ("target", ".git", "generated_fixtures")]
    for fn in fns:
        if not fn.endswith(".rs"):
            continue
        p = os.path.join(dp, fn)
        rel = os.path.relpath(p, root)
        g = group(rel)
        if g is None:
            continue
        prod, test = split(p)
        tot[g][0] += prod
        tot[g][1] += test
src_total = sum(p + t for g, (p, t) in tot.items() if g not in ("al-syntax", "tests/", "benches", "tools"))
print(f"src/ total: {src_total}")
for g, (p, t) in sorted(tot.items(), key=lambda kv: -(kv[1][0] + kv[1][1])):
    print(f"{g:24s} total {p+t:7d}  prod {p:7d}  test {t:7d}")
