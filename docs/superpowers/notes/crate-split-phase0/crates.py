"""Phase 0: commits per planned crate (design section 4.2 mapping, candidate A).

Usage: git log --since=2026-09-10 --no-merges --first-parent? (see note) --format='@%H' --name-only master -- src crates
       | python3 crates.py
A commit counts once per planned crate it touches. Only paths under src/ and
crates/al-syntax/ are mapped; tests/ and docs are ignored (as in the third review).
"""
import sys, collections, re

PROGRAM_ROOTS = {"types", "source_text", "capped_io", "big_stack", "record_ops", "census_hook", "git_env",
                 "state_paths", "protocol", "config", "telemetry", "app_image", "app_package", "dependencies",
                 "language", "release_alloc", "test_quality"}
ENGINE_LOW = {"ids", "perf_trace", "census", "root_classification", "snapshot"}
GATE_LOW = {"version", "model_instance_id", "workspace_diagnostics", "app_attribution"}
CORE_VOCAB = {"follow_up", "result_header", "result_index", "line_base", "joined_key", "path_segments",
              "source_excerpt", "return_summary", "vocab"}
L5_COMMANDS = {"digest_cli", "digest_v3", "digest_v4", "digest_v5", "explain", "prove", "prove_slice",
               "prove_slice_tests", "fingerprint_cli", "fingerprint_query", "callers_context",
               "callers_context_tests", "emit_policy", "snapshot_full", "test_coverage", "selector_index",
               "routine_rollup", "field_guard", "diff_parser", "no_path_tests", "group_path_cli"}
DET_CORE = {"d1", "mod", "d65"}
ENGINE_UPPER = {"batch", "session", "session_phase_tests", "incremental", "switch_dump", "resident", "impact",
                "test_select", "host"}

def stem(p):
    return re.sub(r"\.rs$", "", p.split("/")[-1])

def crate(p):
    if p.startswith("crates/al-syntax/"):
        return "al-syntax"
    if not p.startswith("src/"):
        return None
    parts = p[4:].split("/")
    top = re.sub(r"\.rs$", "", parts[0])
    if top in ("lsp",):
        return "al-program" if p == "src/lsp/encoding.rs" else "facade"
    if top in ("mcp", "analysis", "bin", "main", "server", "watcher", "lib", "cdo_gate_coverage_tests", "test_alloc"):
        return "facade"
    if top == "wire":
        return "al-engine (upper)"
    if top == "snapshot":
        return "al-program"
    if top == "program":
        return "al-engine (upper)" if len(parts) > 1 and parts[1] == "impact_graph" else "al-program"
    if top in PROGRAM_ROOTS:
        return "al-program"
    if top == "engine":
        if len(parts) == 1:
            return "facade"
        sub = re.sub(r"\.rs$", "", parts[1])
        if sub == "mod":
            return "al-program"
        if sub in ENGINE_LOW:
            return "al-program"
        if sub in CORE_VOCAB:
            return "al-engine-core (l5 substrate)"
        if sub in ENGINE_UPPER:
            return "al-engine (upper)"
        if sub == "deps":
            return "al-engine (upper)" if stem(p) in ("dep_artifact_l4", "r3a4_projection") else "al-program"
        if sub == "gate":
            s = stem(p)
            if s in GATE_LOW:
                return "al-program"
            if s in ("cbor", "format_json"):
                return "al-engine-core (l5 substrate)"
            return "al-engine (upper)"
        if sub == "l4":
            s = stem(p)
            if s in ("combined_graph", "scc"):
                return "al-program"
            if s == "effect_query_cli":
                return "al-engine (l5 commands)"
            return "al-engine-core (l4)"
        if sub == "l5":
            if len(parts) > 2 and parts[2] == "detectors":
                return "al-engine-core (l5 substrate)" if stem(p) in DET_CORE else "al-detectors"
            s = parts[2].replace(".rs", "")
            if s in L5_COMMANDS:
                return "al-engine (l5 commands)"
            return "al-engine-core (l5 substrate)"
        return "unmapped:" + p
    return "unmapped:" + p

per = collections.Counter()
only = collections.Counter()
combos = collections.Counter()
unmapped = collections.Counter()
lowest = collections.Counter()
# candidate A's chain, bottom first: an edit rebuilds its lowest crate and everything above
CHAIN = ["al-syntax", "al-program", "al-engine-core", "al-detectors", "al-engine", "facade"]
n = 0
cur = None

def flush(s):
    global n
    if s is None:
        return
    s.discard(None)
    um = {c for c in s if c.startswith("unmapped:")}
    for c in um:
        unmapped[c] += 1
    s -= um
    if not s:
        return
    n += 1
    for c in s:
        per[c] += 1
    layers = {c.split(" ")[0] for c in s}
    combos[tuple(sorted(layers))] += 1
    lowest[min(layers, key=CHAIN.index)] += 1

for line in sys.stdin:
    line = line.strip()
    if line.startswith("@"):
        flush(cur)
        cur = set()
    elif line:
        cur.add(crate(line))
flush(cur)
print(f"commits touching src/ or al-syntax: {n}")
for c, k in per.most_common():
    print(f"  {c:32s} {k:4d}  ({100*k/n:.0f}%)")
print("by set of crates touched (layer level):")
for c, k in combos.most_common(15):
    print(f"  {k:4d}  {' + '.join(c)}")
print("by LOWEST crate touched (what candidate A rebuilds from):")
for c in CHAIN:
    print(f"  {c:16s} {lowest[c]:4d}  ({100*lowest[c]/n:.0f}%)")
if unmapped:
    print("unmapped:", unmapped.most_common(10))
