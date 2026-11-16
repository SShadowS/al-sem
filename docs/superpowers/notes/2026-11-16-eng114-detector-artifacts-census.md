# ENG-114: the detector artifacts and the sited graph, split

**Measured** 2026-11-16 on the pinned CDO baseline (`/work/src/cdo-baseline/Cloud`, `bc3ccb18`),
master `82b75594` plus this branch's census additions (no behaviour change: a read-only
`ConeDerivedStore` census hook and probe code), `tools/census-probe resident --sited`, release,
one run. Output: `tools/census-probe/runs-eng114/resident-split-sited.out`. `--sited` builds the
sited graph from the same program the way the session does (`build_target_with` + `build_sited`,
2.0 s) and attaches it with `Revision::with_sited`, so this run's live total (4,220.5 MiB) is
not comparable with censuses made without it; the default stays without.

Accounting as in ENG-107: drop deltas, a shared allocation on its last holder, counts taken
before any drop, MiB = 1,048,576 bytes, allocator overhead excluded.

## What the three detector buckets and the sited graph hold

| Bucket | Part | MiB | Allocations |
|---|---|---:|---:|
| `det.cone_derived` (93.4) | `events_pool` (15,531,457 ids of 4 B, capacity 16,777,216) | **64.00** | 2 |
| | `phys_writes_pool` / `phys_reads_pool` / `writes_all_pool` | 8.00 / 8.00 / 4.00 | |
| | rows (34,203, 36 B, keys 2.64 MiB) / interner (11,146 strings, stored twice) | 6.70 / 2.73 | |
| `det.summaries` (85.1) | coverage records | **38.78** | 435,756 |
| | direct facts (46,101 at 416 B) | 29.01 | 181,483 |
| | the map and its ids | 17.35 | 68,407 |
| `det.transaction_spans` (60.4, 64 spans) | `publishes_events` (299,958 strings, 8,618 distinct texts) | **32.56** | 300,014 |
| | `pending_events` (221,464 strings, 8,590 distinct) | **24.04** | 221,514 |
| | the rest (routines, tables, roots, ids, other lists) | 3.81 | |
| sited graph (56.5, not measured before) | `node_ix` (a hash map node key → index) | **20.38** | 4,207 |
| | sites / nodes / stable ids / names | 11.61 / 9.22 / 7.08 / 3.75 | |

The capacity lines say what the pools waste: `events_pool` 1.25 M unused slots (4.8 MiB),
`phys_reads_pool` 0.64 M (2.4 MiB), `phys_writes_pool` 0.2 M (1.6 MiB), the doubling slack of a
pushed `Vec`.

## Levers, with bars

1. **Transaction spans' event names: share the texts (do in ENG-114).** 521,422 event-name
   strings over about 8,600 distinct texts hold 56.6 MiB; as shared strings (`SharedStr`, 16 B
   each, one text per name) they hold about 8 MiB plus 0.7 MiB of text. **Bar: at least 45 MiB
   live.** Output: the strings compare, sort, hash and print as before (`SharedStr` is the text).
2. **`det.cone_derived`'s event pool (file as a task).** 15.5 M ids for 34,203 routines, 454 a
   routine: every routine's window lists the events of its whole cone, and routines that reach
   the same publishers repeat the same window. How many windows are distinct is not measured: the
   task's first step counts them; sharing identical windows (one copy, many rows) could save most
   of the 64 MiB. Exact-size pools alone save about 9 MiB. Bar: set from the distinct-window count.
3. **`det.summaries`' coverage records (fold into ENG-108).** 38.78 MiB in 435,756 allocations
   for 34,203 routines, about 13 allocations a record: the same shape as `fold.coverage` (46.57
   MiB) and `tree_base.coverage` (53.70 MiB), the coverage strings ENG-108 was re-scoped to cover.
   A third copy of them, so ENG-108's census should include it.
4. **`det.summaries`' direct facts (file as a task).** 46,101 facts, 29.01 MiB, held by value. The
   fold keeps its own direct facts (`fold.direct_full`, 56.10 MiB); whether these are copies of
   those (shareable as `Arc<CapabilityFact>`) is not measured here. Bar: from that check.
5. **The sited graph's `node_ix` (file as a task).** A 20.38 MiB hash map from node key to index
   beside the 9.22 MiB node list; if the nodes are sorted by key (or can be), a binary search
   replaces it. Bar: at least 15 MiB, after checking the lookup's hot-path cost.

## Limits

- One run. Byte counts are deterministic for one input.
- The summaries' "rest" line holds the map, its keys and every summary's routine id; not split.
- Lever 2's saving depends on a count this census does not make.
