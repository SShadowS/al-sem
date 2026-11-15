"""ENG-84: CPU shares from a `samply record --unstable-presymbolicate` profile.

Usage: shares.py <prof.json.gz> <prof.json.syms.json> [needle ...] [callers=<needle> ...]
Weights each sample by its thread CPU delta (CPU time, not wall). Prints the top functions by
self share (leaf) and by inclusive share (anywhere on the stack, inlined frames included), then
for each needle the largest inclusive share of any name containing it. `callers=<needle>` also
lists who calls the matching functions (the first non-generic frame above the first match).
Inclusive shares need a frame-pointer build (`RUSTFLAGS='-C force-frame-pointers=yes'`).
"""
import bisect
import gzip
import json
import re
import sys
from collections import defaultdict

prof = json.load(gzip.open(sys.argv[1]))
syms = json.load(open(sys.argv[2]))
needles = [n for n in sys.argv[3:] if not n.startswith("callers=")]
caller_needles = [n[len("callers="):] for n in sys.argv[3:] if n.startswith("callers=")]
callers = {n: defaultdict(float) for n in caller_needles}
strings = syms["string_table"]
tables = {}
for d in syms["data"]:
    # Each sampled address maps to its own entry (with that address's inlined frames).
    tables[d["debug_name"]] = (
        d["symbol_table"],
        {addr: ix for addr, ix in d["known_addresses"]},
    )

shared = prof.get("shared", {})
shared_strings = shared.get("stringArray", [])
libs = prof["libs"]


def clean(name):
    name = re.sub(r"::h[0-9a-f]{16}$", "", name)
    return name


def names_at(lib, addr):
    """Outermost-last list of (inlined ... outer) names for an address in a library."""
    t = tables.get(lib)
    if t is None or addr < 0:
        return []
    st, known = t
    i = known.get(addr)
    if i is None:
        return []
    s = st[i]
    out = [
        clean(strings[f["function"]])
        for f in s.get("frames", [])
        if f.get("function") is not None
    ]
    out.append(clean(strings[s["symbol"]]))
    return out


selfw = defaultdict(float)
inclw = defaultdict(float)
total = 0.0
for th in prof["threads"]:
    samples = th["samples"]
    if samples.get("length", 0) == 0:
        continue
    ft, fn, stk, res = th["frameTable"], th["funcTable"], th["stackTable"], th["resourceTable"]
    strs = th.get("stringArray") or shared_strings
    cache = {}

    def frame_names(f):
        if f in cache:
            return cache[f]
        func = ft["func"][f]
        r = fn["resource"][func]
        addr = ft["address"][f]
        names = []
        if r >= 0:
            lib = libs[res["lib"][r]]["debugName"]
            names = names_at(lib, addr)
        if not names:
            names = [strs[fn["name"][func]] if fn["name"][func] < len(strs) else "?"]
        cache[f] = names
        return names

    stack_cache = {}

    def stack_names(s):
        if s in stack_cache:
            return stack_cache[s]
        leaf = None
        seen = []
        cur = s
        while cur is not None:
            names = frame_names(stk["frame"][cur])
            if leaf is None:
                leaf = names[0]
            seen.extend(names)
            cur = stk["prefix"][cur]
        # For each caller needle: the first frame above the first match that is not itself a match
        # nor a generic wrapper (the function that asked).
        found = {}
        for needle in caller_needles:
            for i, n in enumerate(seen):
                if needle in n:
                    for m in seen[i + 1:]:
                        if needle not in m and "<" not in m and not re.match(
                            r"(core|alloc|std|hashbrown)::|\{closure|hash_one|hash$|write|fold$|next$|from_iter$", m
                        ):
                            found[needle] = m
                            break
                    break
        stack_cache[s] = (leaf, set(seen), found)
        return stack_cache[s]

    deltas = samples.get("threadCPUDelta") or [1] * samples["length"]
    for s, w in zip(samples["stack"], deltas):
        if s is None or not w:
            continue
        leaf, names, found = stack_names(s)
        total += w
        selfw[leaf] += w
        for n in names:
            inclw[n] += w
        for needle, caller in found.items():
            callers[needle][caller] += w

print(f"CPU total {total / 1e6:.1f} s (thread CPU deltas, us)")
print("\n== top 40 self")
for n, w in sorted(selfw.items(), key=lambda kv: -kv[1])[:40]:
    print(f"{100 * w / total:6.2f}%  {n[:140]}")
print("\n== top 60 inclusive")
for n, w in sorted(inclw.items(), key=lambda kv: -kv[1])[:60]:
    print(f"{100 * w / total:6.2f}%  {n[:140]}")
for needle, by_caller in callers.items():
    print(f"\n== callers of {needle} (share of all CPU)")
    for n, w in sorted(by_caller.items(), key=lambda kv: -kv[1])[:12]:
        print(f"{100 * w / total:6.2f}%  {n[:140]}")
if needles:
    print("\n== needles (inclusive: the largest share of any name containing the needle)")
    for needle in needles:
        hits = {n: w for n, w in inclw.items() if needle in n}
        top = sorted(hits.items(), key=lambda kv: -kv[1])[:3]
        print(f"{needle:<40} max {100 * (top[0][1] if top else 0) / total:6.2f}%  e.g. {[t[0][:80] for t in top[:2]]}")
