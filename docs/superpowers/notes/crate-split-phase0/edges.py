"""Phase 0: module edges and back-edges for candidate A (design sections 2 and 6).

Production code only (the inline-test tail and *test*.rs files are skipped, as in lines.py).
Every `crate::a::b…` path (use trees expanded) is resolved to the deepest existing module
file under src/, and both ends are mapped to planned crates with crates.py's mapping.
Prints: module-group SCC size, the edge count between planned crates, and the upward
(back) edges per crate pair, with the distinct source files of each.
Usage: python3 edges.py <repo>
"""
import os, re, sys, collections
sys.path.insert(0, os.path.dirname(__file__))
import importlib.util
spec = importlib.util.spec_from_file_location("crates_map", os.path.join(os.path.dirname(__file__), "crates.py"))
src_text = open(spec.origin).read().split("per = collections.Counter()")[0]
ns = {}
exec(src_text, ns)
crate_of = ns["crate"]
CHAIN = ["al-syntax", "al-program", "al-engine-core", "al-detectors", "al-engine", "facade"]

root = sys.argv[1]
SRC = os.path.join(root, "src")

def prod_text(path):
    lines = open(path, encoding="utf-8", errors="replace").read().split("\n")
    base = os.path.basename(path)
    if is_test_file(base):
        return ""
    for i, l in enumerate(lines):
        if l.strip().startswith("#[cfg(test)]"):
            nxt = " ".join(x.strip() for x in lines[i + 1:i + 3])
            if re.match(r"(pub(\([^)]*\))? )?mod \w+", nxt):
                lines = lines[:i]
                break
    # doc and line comments: intra-doc links are not compile edges
    return "\n".join(l for l in lines if not l.lstrip().startswith("//"))

def is_test_file(base):
    return base == "tests.rs" or base.endswith("_tests.rs") or "_tests_" in base

def expand(tree):
    """Expand a use tree string like 'a::{b, c::{d, e}}' into paths."""
    tree = tree.strip()
    i = tree.find("{")
    if i < 0:
        return [tree.split(" as ")[0].strip()]
    prefix = tree[:i]
    depth, start, items = 0, i + 1, []
    for j in range(i, len(tree)):
        ch = tree[j]
        if ch == "{":
            depth += 1
        elif ch == "}":
            depth -= 1
            if depth == 0:
                items.append(tree[start:j])
                break
        elif ch == "," and depth == 1:
            items.append(tree[start:j])
            start = j + 1
    out = []
    for it in items:
        it = it.strip()
        if not it:
            continue
        for p in expand(it):
            out.append(prefix + p if p != "self" else prefix.rstrip(":"))
    return out

def module_file(parts):
    """Deepest existing module file for crate path parts."""
    best = None
    cur = SRC
    for k, p in enumerate(parts):
        f1 = os.path.join(cur, p + ".rs")
        d = os.path.join(cur, p)
        if os.path.isfile(f1):
            best = f1
            cur = d
        elif os.path.isfile(os.path.join(d, "mod.rs")):
            best = os.path.join(d, "mod.rs")
            cur = d
        else:
            break
    return best

USE = re.compile(r"\buse\s+crate::([^;]+);", re.S)
INLINE = re.compile(r"(?<![\w:])crate::((?:\w+::)*\w+)")

edges = collections.Counter()
back = collections.defaultdict(set)
grp_edges = collections.defaultdict(set)

def group_of(rel):
    # module group = path up to 3 components (e.g. engine/l5/detectors), file stem dropped
    parts = rel[4:].replace(".rs", "").split("/")
    if parts[-1] == "mod":
        parts = parts[:-1]
    return "/".join(parts[:3])

for dp, dns, fns in os.walk(SRC):
    for fn in fns:
        if not fn.endswith(".rs"):
            continue
        path = os.path.join(dp, fn)
        rel = os.path.relpath(path, root)
        a = crate_of(rel)
        if a is None or a.startswith("unmapped"):
            continue
        a = a.split(" ")[0]
        text = prod_text(path)
        targets = []
        for m in USE.finditer(text):
            targets += expand(m.group(1).replace("\n", " "))
        text_wo = USE.sub("", text)
        targets += [m.group(1) for m in INLINE.finditer(text_wo)]
        for t in targets:
            parts = [p for p in t.split("::") if p and p != "*"]
            mf = module_file(parts)
            if mf is None:
                continue
            trel = os.path.relpath(mf, root)
            b = crate_of(trel)
            if b is None or b.startswith("unmapped"):
                continue
            b = b.split(" ")[0]
            grp_edges[group_of(rel)].add(group_of(trel))
            if a != b:
                edges[(a, b)] += 1
                if CHAIN.index(b) > CHAIN.index(a):
                    back[(a, b)].add(rel + " -> " + trel)

# SCC over module groups (Tarjan, iterative enough for this size)
sys.setrecursionlimit(100000)
idx, low, on, st, sccs, counter = {}, {}, set(), [], [], [0]
def strong(v):
    idx[v] = low[v] = counter[0]; counter[0] += 1; st.append(v); on.add(v)
    for w in grp_edges.get(v, ()):
        if w not in idx:
            strong(w); low[v] = min(low[v], low[w])
        elif w in on:
            low[v] = min(low[v], idx[w])
    if low[v] == idx[v]:
        comp = []
        while True:
            w = st.pop(); on.discard(w); comp.append(w)
            if w == v:
                break
        sccs.append(comp)
nodes = set(grp_edges) | {w for ws in grp_edges.values() for w in ws}
for v in sorted(nodes):
    if v not in idx:
        strong(v)
big = max(sccs, key=len)
print(f"module groups: {len(nodes)}; largest SCC: {len(big)}; groups outside it: {len(nodes) - len(big)}")
print("path references between planned crates (production code):")
for (a, b), k in sorted(edges.items(), key=lambda kv: (CHAIN.index(kv[0][0]), CHAIN.index(kv[0][1]))):
    tag = "BACK" if CHAIN.index(b) > CHAIN.index(a) else ""
    print(f"  {a:16s} -> {b:16s} {k:5d} {tag}")
print("back-edges (distinct file pairs) per crate pair:")
tot = 0
for (a, b), s in sorted(back.items(), key=lambda kv: -len(kv[1])):
    files = {x.split(' -> ')[0] for x in s}
    tot += len(s)
    print(f"  {a} -> {b}: {len(s)} file pairs from {len(files)} files")
print(f"total back-edge file pairs: {tot}")
if len(sys.argv) > 2:
    for (a, b), s in sorted(back.items()):
        for x in sorted(s):
            print(f"{a}->{b}\t{x}")
