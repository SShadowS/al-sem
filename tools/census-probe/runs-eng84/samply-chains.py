"""Where a leaf function's CPU sits in a samply profile, by the chain of frames above it.

Usage: samply-chains.py <prof.json.gz> <prof.json.syms.json> <needle> [depth]

The input is a profile recorded with `samply record --unstable-presymbolicate` (as
`scripts/profile` does) and its `.syms.json` sidecar. For every sample whose stack holds a frame
whose name contains <needle>, the script takes the `depth` frames (default 6) above the outermost
such frame. Inlined frames are included and generic arguments are folded to <..>. Each sample is
weighted by its thread CPU delta.

It prints each chain's share of all CPU. Started as ENG-98's SipHash reader, with the needle made
an argument for TLS-86 (`docs/superpowers/notes/2026-11-16-tls86-profiles.md`).

A frame just above a libc leaf can be wrong: TLS-86 saw one lookup per root credited with 26 %.
Check an unlikely chain with a second tool before acting on it.
"""
import gzip
import json
import re
import sys
from collections import defaultdict

prof = json.load(gzip.open(sys.argv[1]))
syms = json.load(open(sys.argv[2]))
needle = sys.argv[3]
depth = int(sys.argv[4]) if len(sys.argv) > 4 else 6
strings = syms["string_table"]
tables = {d["debug_name"]: (d["symbol_table"], {a: i for a, i in d["known_addresses"]}) for d in syms["data"]}
libs = prof["libs"]
shared_strings = prof.get("shared", {}).get("stringArray", [])


def clean(n):
    n = re.sub(r"::h[0-9a-f]{16}$", "", n)
    return re.sub(r"<[^<>]*(<[^<>]*(<[^<>]*>[^<>]*)*>[^<>]*)*>", "<..>", n)


def names_at(lib, addr):
    t = tables.get(lib)
    if t is None or addr < 0:
        return []
    st, known = t
    i = known.get(addr)
    if i is None:
        return []
    s = st[i]
    out = [strings[f["function"]] for f in s.get("frames", []) if f.get("function") is not None]
    out.append(strings[s["symbol"]])
    return out


chains = defaultdict(float)
total = hit = 0.0
for th in prof["threads"]:
    samples = th["samples"]
    if samples.get("length", 0) == 0:
        continue
    ft, fn, stk, res = th["frameTable"], th["funcTable"], th["stackTable"], th["resourceTable"]
    strs = th.get("stringArray") or shared_strings
    fcache = {}

    def frame_names(f):
        if f not in fcache:
            func = ft["func"][f]
            r = fn["resource"][func]
            names = names_at(libs[res["lib"][r]]["debugName"], ft["address"][f]) if r >= 0 else []
            fcache[f] = names or [strs[fn["name"][func]] if fn["name"][func] < len(strs) else "?"]
        return fcache[f]

    scache = {}

    def chain(s):
        if s in scache:
            return scache[s]
        seen = []
        cur = s
        while cur is not None:
            seen.extend(frame_names(stk["frame"][cur]))
            cur = stk["prefix"][cur]
        last = max((i for i, n in enumerate(seen) if needle in n), default=None)
        scache[s] = None if last is None else " <- ".join(clean(n) for n in seen[last + 1 : last + 1 + depth])
        return scache[s]

    deltas = samples.get("threadCPUDelta") or [1] * samples["length"]
    for s, w in zip(samples["stack"], deltas):
        if s is None or not w:
            continue
        total += w
        c = chain(s)
        if c is not None:
            hit += w
            chains[c] += w

print(f"CPU total {total / 1e6:.1f} s, under {needle} {100 * hit / total:.2f}%")
for c, w in sorted(chains.items(), key=lambda kv: -kv[1])[:15]:
    print(f"{100 * w / total:6.2f}%  {c[:330]}")
