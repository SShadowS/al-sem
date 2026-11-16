# ENG-107: the three unsplit resident buckets, split

**Measured** 2026-11-16 on the pinned CDO baseline (`/work/src/cdo-baseline/Cloud`, `bc3ccb18`),
master `bbcbd603` plus this branch's census hooks (no behaviour change), `tools/census-probe
resident`, release, one run. Output: `tools/census-probe/runs-eng107/resident-split.out`. Live
after one ask of each kind: 4,676.7 MiB. The survey estimates quoted below come from Fable's
optimization survey 2 (2026-11-16; read-only, estimates; the copy the coord tasks name).

Accounting: each line is a drop delta, the bytes and allocations freed by dropping that part.
A shared allocation falls to the LAST holder dropped. Counts are taken before anything drops.
MiB = 1,048,576 bytes; allocator overhead is not included. Drop orders:
- **`fold.cones`:** the emit source's cone list, the records' fact cones, the records' coverage
  cones, the record map, then the emit source's graph, SCCs, `succ`, frames and `changed`. The
  fact cones are held by the records and the emit source, so they fall on the records' line.
- **`artifact.det`:** in the order of its table rows below (the probe's `eng107_split`).
- **`revision.store`:** the tree, then its base (the tree shares sections with the base), then
  the sited graph.

## What the three buckets hold

| Bucket (before) | Part | MiB | Allocations |
|---|---|---:|---:|
| `fold.cones` (747.62) | fact cones (`SccRecord.fact`) | **666.43** | 1,962,468 |
| | coverage cones (`SccRecord.cov`) | 11.28 | 130,964 |
| | record map (table and digests) | 7.06 | 2 |
| | emit source: typed-edge graph out-edges / nodes / unresolved | 42.93 / 3.43 / 0.00 | 298,766 / 34,207 / 39 |
| | emit source: SCCs / `succ` / frames / `changed` | 12.42 / 2.71 / 1.12 / 0.00 | 101,640 / 19,115 / 12,367 / 1 |
| | emit source: its cone list (33,231 slots; no cone is held only there) | 0.25 | 2 |
| `revision.store` (610.66) | the tree (`CapabilitySnapshot`): `pd_solution` | **253.00** | 1,239,250 |
| | the tree: callsite index / operation index / facts / commit scopes / own `boundary` | 28.39 / 11.12 / 9.17 / 0.09 / 5.68 | |
| | the tree base: `view` | **103.03** | 942,437 |
| | the tree base: coverage / typed edges / callsite resolutions | 53.70 / 50.95 / 47.49 | |
| | the tree base: routine-order frames / event declarations / root classes / identities / boundary | 17.75 / 8.89 / 8.33 / 7.36 / 5.68 | |
| | the sited graph | not measured (see Limits) | |
| `artifact.det` (313.40) | `cone_derived` | **93.43** | 56,503 |
| | `summaries` | 80.44 | 685,644 |
| | `transaction_spans` | 60.41 | 569,182 |
| | upgraded bindings / event graph / resolved call edges / parameter roles | 19.87 / 12.24 / 10.52 / 9.98 | |
| | reverse call graph / event-flow indexes / root classes / dependency routine ids | 8.88 / 7.94 / 5.61 / 2.94 | |

The fact-cone and coverage-cone lines are shown with the hook's own bytes taken out: the hook
moves the cones into two new exact-size lists (33,231 × 8 B = 0.25 MiB each), freed on those two
lines (printed 666.68 and 11.53 MiB). The `fold.cones` parts sum to 748.13 MiB as printed, 0.51
MiB above the old bucket: those two lists and the part boxes. `artifact.det` sums to 313.40 MiB,
as before; the tree (307.46) and its base (303.19) make the old `revision.store` (610.66, now
0.00), and their allocations match it exactly.

Counts behind the cone lines (taken before any drop): 33,231 SCC records, 30,609 with a fact cone
and a coverage cone; 30,609 distinct fact cones, every one also held by the emit source;
5,612,960 fact-cone entries at 56 B (key `Arc<str>` 16 + `ConeFactEntry` 40); 306,465 distinct
keys, 30.35 MiB of text. Coverage cones: 4,403 reason strings (3 distinct texts) and 89,771
unknown-target strings (188 distinct texts, 6.93 MiB of heap).

## What it means for the queued tasks

- **ENG-108, coverage cones as bitsets: the retained cones are small; the copies are not
  measured.** The retained coverage cones hold 11.28 MiB, so a layout change to `CoverageCone`
  alone saves at most that (the survey estimated 50-200 MiB). But `coverage_record`
  (`capability_cone.rs`, `fn coverage_record`) clones each cone's `reasons` and
  `unknown_targets` into every member routine's row, and those rows are retained twice:
  `fold.coverage` (46.57 MiB, 469,960 allocations) and the tree base's `coverage` (53.70 MiB,
  435,758 allocations). Those ~100 MiB hold copies of the same 188 + 3 texts; how much of them is
  those strings is not measured here. The fold's transient unions are not measured either (a
  resident census cannot see a build transient). **Recommendation:** re-scope ENG-108 to the
  coverage strings end to end (the cones and both row stores), share the 191 texts, and census
  `fold.coverage` and `tree_base.coverage` by field first. Bar: set from that census; a change to
  the retained cones alone is not worth an M task.
- **ENG-109, frozen fact cones as flat arrays: larger than estimated, bar from a ceiling.** The
  fact cones are 666.43 MiB. Their payload as a flat array is 5,612,960 × 56 B = 299.76 MiB, and
  the keys' 30.35 MiB of text stays wherever the entries live. That leaves **at most 336.3 MiB**
  for the B-tree layout, and it is a ceiling: the remainder also holds what only the entries
  own, namely the `Arc<CapabilityFact>` reps (a deep clone per direct fact, a separate copy from
  `fold.direct_full`), one joined `rep_key` per direct fact, the guards, and the 306,465 key `Arc`
  headers (about 4.7 MiB). An estimate from the allocation count: 1,962,468 minus 306,465 keys
  minus 30,609 cone `Arc`s leaves about 1.63 M; at two-thirds fill the nodes are about 0.85 M, so
  roughly 0.7 M allocations are entry-owned heap that a flat array keeps. Narrowing `dist` to
  `u32` alone saves **nothing** (36 B pads back to 40); it saves 21 MiB only if `dist` becomes
  its own column, and 43 MiB only if a second 4 B field (such as `guard` as a `u32` index) pairs
  with it. **Bar: at least 150 MiB live**, to be confirmed by ENG-109's own first census, which
  splits the nodes from the entries' heap.
- **ENG-111, tree and tree-base stable-id strings: two large parts.** The tree's `pd_solution`
  (253.00 MiB, 1.24 M allocations; the survey's `derive_pd_arena`, which clones a stable id into
  every state) and the base's `view` (103.03 MiB, 0.94 M allocations) are 356 MiB between them.
  How much is repeated id text is not measured: ENG-111's first step is a census of the strings
  in `pd_solution` and `view` (count, distinct, bytes). **Bar: at least 100 MiB live**, confirmed
  or adjusted by that census. The sited graph was not measured (see Limits); read it before
  choosing what to share, as the task says.
- **ENG-112, dense node ids and CSR in the cone layer: memory is modest.** The cone layer's graph
  is 61.5 MiB (out-edges 42.93, SCCs 12.42, nodes 3.43, `succ` 2.71), plus the detector context's
  event graph (12.24) and reverse call graph (8.88). Memory alone does not justify an M-L change.
  **Bar:** the CPU case (survey: 3-6 % of analyze CPU, not measured here), at least 3 % measured
  twice, with at least 30 MiB live as a secondary.
- **ENG-113, memory-mapped dependency source text: the bucket is unchanged.** `bodies: source
  text + paths + provenance` is 120.37 MiB (76,168 allocations), the same as in ENG-94's run.
  **Bar:** this census bucket falls by at least 100 MiB, and `/proc/<pid>/smaps` shows the text
  as shared file-backed pages across two engine processes (the census cannot see a mapping, and
  RSS alone is not size evidence).

## New candidates (not in survey 2)

- In `artifact.det`: `cone_derived` (93.43 MiB in 56,503 allocations, about 1.7 KiB each on
  average; whether that is a few large blocks is an inference, not measured), `summaries` (80.44
  MiB) and `transaction_spans` (60.41 MiB in 569,182 allocations).
- The coverage rows: `fold.coverage` (46.57 MiB) and the tree base's `coverage` (53.70 MiB), see
  ENG-108.

## Limits

- One run. The bytes are deterministic counts, but which artifacts are filled follows the
  probe's fixed asks.
- **The sited graph is not measured.** The probe builds its revision without seeding one, and a
  revision refuses to fill `Sited` afterwards (`src/engine/resident/revision.rs`), so no ask mix
  fills it. Measuring it needs the probe to seed one at construction. `SitedGraph`'s census hook
  compiles but has not run.
- `det.graph` and `det.cone` print 0.00 MiB: they are `Arc`s another holder still keeps, so
  their bytes fall on a later line (not the det total).
- The tree's sections it shares with its base are dropped with the tree's own `boundary` as one
  line (5.68 MiB, the tree's own boundary); the shared sections' bytes fall on the base lines.
- The B-tree node bytes inside the fact cones are not split from the entries' own heap.
- The CPU figures quoted as bars (ENG-108, ENG-112) have no artifact here.
