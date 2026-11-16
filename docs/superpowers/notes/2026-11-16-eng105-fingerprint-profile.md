# ENG-105: witness reconstruction in `alsem fingerprint`, profiled and timed

Recorded 2026-11-16 on server1, the pinned CDO baseline (`bc3ccb18`), in the CDO slot.
Base: master `bbcbd603` (after ENG-100, ENG-99, ENG-102). Head: `4c3cc0d5` (ENG-105).
Command: `alsem fingerprint /work/src/cdo-baseline/Cloud`, default options.

## Profiles (samply, `profiling` builds, CPU = every thread's CPU, summed)

| | base | head |
|---|---:|---:|
| CPU total | 107.8 s | 91.5 s |
| `reconstruct_witness_paths`, inclusive | 60.9 % | 56.2 % |
| `HopCache::get`, inclusive | 10.2 % | 4.0 % |
| SipHash (`write<Sip13Rounds>`), inclusive | 6.4 % | 3.2 % |

Where the base's SipHash sat (callers, share of all CPU, the post-ENG-100 profile): the
`HopCache` map keyed by edge pointer (`hash_one<RandomState, &*const SnapshotGraphEdge>`
2.43 % + 1.35 % in SipHash), `enumerate_proofs`' `seen: HashSet<(Vec<usize>, usize)>`
(~2.1 % with its rehash), and `&str` maps under `descend` (0.82 %) and
`terminal_hop_from_fact` (0.39 %). The candidate sort (`quicksort<Candidate>`) was 6.4 %
inclusive, `key_cmp` 1.4 % self.

## What changed, and what did not work

- `HopCache` keeps its memos in a slot per edge index of the one `typed_edges` slice it serves
  (its start and length kept; any other edge uses the address map). Kept.
- `enumerate_proofs`' `seen` set (only inserted into) uses FxHash. Kept.
- A rank-keyed candidate sort (each hop's text ranked once, then a stable sort by the ranks) was
  tried and REMOVED: it made `fingerprint` 2x slower (user 188-190 s against 86-100 s;
  `sort_candidates` 57 % of the head's CPU). The ranks called `json()` on every hop of every
  candidate; `json()` is built lazily, and `key_cmp` skips shared hops by `Rc::ptr_eq` and
  serializes a hop only at a real tie, so the cached keys built JSON the old sort never needed.
  A faster sort needs keys that do not force the JSON (for example a per-hop id assigned in
  build order, compared only where `key_cmp` would compare texts); not attempted.

## Timed (release-fast binaries from `cdo-before-after`, `/usr/bin/time -v`, alternating)

| round | base CPU (user + sys) | head CPU | head / base | base wall | head wall |
|---|---:|---:|---:|---:|---:|
| 1 | 95.7 s | 89.6 s | x0.936 | 1:09.9 | 1:04.6 |
| 2 | 118.2 s | 102.2 s | x0.865 | 1:31.8 | 1:14.5 |
| 3 | 99.2 s | 91.4 s | x0.921 | 1:12.8 | 1:04.9 |
| 4 (head first) | 99.0 s | 91.2 s | x0.921 | 1:12.8 | 1:05.0 |

Round 2 ran under other load (base 24 % slower than its other runs). Rounds 3 and 4 agree:
CPU **x0.92**, wall about **x0.89**. Peak RSS unchanged (7.9 GB). The outputs are byte-identical.
`cdo-speed` twice, both PASS: build x1.019 / x0.907 (noise: it does not run this code), warm
prove x0.810 / x0.950, peak memory x0.957 / x0.969. `cdo-before-after bbcbd603 4c3cc0d5`:
nothing moved.

## Raw files

Profiles and timing files are in the session scratchpad of the tools lane (not committed:
`e105c-prof-{base,head}.json.gz` with `.syms.json`, `e105c-fp-*-*.time`). TLS-86 is to commit
the ENG-98/100 profiles; this note records the numbers.
