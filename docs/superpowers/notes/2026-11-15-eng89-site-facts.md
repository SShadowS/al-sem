# ENG-89: what the 385 MiB of dependency site facts is

Measured 2026-11-15 on server1 (kraken tentacle), master `4dcf4b44`, pinned CDO baseline
(`/work/src/cdo-baseline/Cloud`, `bc3ccb18`), under `scripts/cdo-slot`. Tool:
`tools/census-probe resident` (counting allocator, requested sizes), which now splits
`session.dep_resolution.site_facts` by part; the run is in `tools/census-probe/runs-eng89/`.
Only the dependency side was measured; the workspace's own site facts (in the model build) use
the same type and would gain in proportion.

## Headline

**The bytes are the map's layout, not facts.** `site_facts` is a
`HashMap<ObligationId, SiteFacts>` with 318,982 entries in a table of 524,288 slots (usable
capacity 458,752: hashbrown grows at 7/8, so the table is 61% full). Each slot is a 128 B key
and a 592 B `SiteFacts` plus a control byte, so the table alone is **360.5 MiB** of the measured
384.9 MiB. All the fields' heap together is 7.6 MiB. Most fields are empty on most sites.

## The split

| Part | MiB |
|---|---:|
| table: 524,288 slots × (128 + 592 + 1) B | 360.5 |
| keys' file path (`span.unit`, one `String` per key, 7,003 distinct files) | 16.1 |
| fields' heap (all of them) | 7.6 |
| remainder (heap the probe does not walk) | 0.8 |
| **measured drop** | **384.9** |

Every key is `ObligationId::CallSite`; caller ids are shared strings (`SharedStr`), so they cost
no heap here. `shrink_to_fit` would not help: 318,982 entries need 364,551 slots, which rounds
up to the same 2^19.

`SiteFacts` has 23 fields: `receiver` (`Option<ReceiverFact>`, 56 B), 5 bools, `starts_object`
(a one-byte enum) and 16 others. By use (318,982 sites):

| Field | Sites using it | Heap MiB |
|---|---:|---:|
| `bare_scope_complete` (bool) | 152,861 (48%) | 0 |
| `receiver` (type text and object name) | 141,165 (44%) | 5.6 |
| `unqualified_record_op` (bool) | 9,544 (3.0%) | 0 |
| `validate_field` | 8,146 | 0.1 |
| `starts_object` (not `No`) | 5,944 | 0 |
| `start_method` | 5,212 | 0.0 |
| `run_trigger_excluded` | 3,838 | 1.1 |
| `table_event_raise` | 3,292 | 0 |
| `build_context` (22 distinct contexts) | 2,729 | 0.1 |
| `interface` | 1,198 | 0.0 |
| `run_target` | 928 | 0 |
| `dataset_run` | 649 | 0.5 |
| `open` | 579 | 0.1 |
| `request_page_may_cancel` (bool) | 376 | 0 |
| `record_table_unresolved` | 208 | (not walked) |
| `argument_codeunits` | 93 | 0.0 |
| `field_ref_validate` | 74 | (not walked) |
| `interface_closed_over` | 69 | 0.0 |
| `interface_holder` | 48 | 0.0 |
| `call_result_record`, `inherited_protected` (bool) | 34 each | 0 |
| `interface_unmapped_enums`, `record_op_field_named` (bool) | 0 | 0 |

**31,067 sites (9.7%)** use any field beyond `receiver` and `bare_scope_complete`.

## Who reads it

Lookups by `ObligationId`: `program_calls.rs` (the model's adapter, through `SiteFactsRef`),
`compiler_golden.rs:377` (`ProgramReport.site_facts`, a raw map), `impact_graph/project.rs:553`
and `:687-699` (which rebuilds a `CallSite` key from a `TableEvent` edge's caller, span and
callee fingerprint), `interface_holder.rs:233` and `:320` (other edges' ids), `aldump.rs:1827`.
Writers and raw-map holders: `interface_holder.rs:218` (mutates, `&mut HashMap`),
`session/dep_record.rs:343-368` and `:437`, `full.rs:3285-3303` and `:3382`
(`Arc<HashMap>`). So not every access goes through `SiteFactsMap`/`SiteFactsRef`
(`src/program/resolved.rs`). The census tallies at `resolved.rs:495` and
`session/workspace.rs:758` size `(ObligationId, SiteFacts)`; they encode nothing, but must follow
a layout change. (`prove_slice.rs` has its own, unrelated `SiteFacts`.)

## Shrink proposals

1. **A hot record and a boxed cold part** (the big one). Keep `receiver`, the 5 bools and
   `starts_object` inline, and move the other 15 fields into one
   `Option<Box<RareSiteFacts>>`. The record goes from 592 B to about 72 B (56 + 8 + the small
   fields). In today's table that is 524,288 × 520 B ≈ **260 MiB**, less ~16 MiB of boxes for the
   31,067 sites that have any rare field: **about 244 MiB**.
2. **No second copy of the key.** Every `ClassifiedEdge` already carries its `ObligationId`.
   Facts in a `Vec` indexed by a `u32` on the edge would drop the 128 B key, the table's empty
   39%, and the keys' file paths: after proposal 1, the 100.5 MiB table becomes ~22 MiB of values
   plus 1.7 MiB of edge indices, and 16.1 MiB of paths go too, **about 93 MiB**. The catch: the
   lookups that rebuild or borrow another edge's key (`project.rs:687-699`,
   `interface_holder.rs:320`) need the index on those edges too (`TableEvent`, `Background`).
   The simpler alternative, a `Vec` sorted by key with a binary search, keeps the key and saves
   only the empty slots: about 40 MiB.
3. **Not worth it on its own:** interning `build_context` (22 distinct contexts, 0.1 MiB) and the
   field strings (7.6 MiB in all).

With 1 and 2 the 385 MiB becomes about 48 MiB (values 22, boxes 16, field heap 7.6, edge
indices 1.7, remainder 0.8): **about 337 MiB** off live memory (the census's 5,490 MiB after
the asks, with ENG-86), or ~284 MiB with the sorted-`Vec` variant of 2. After the dependency
syntax trees (728 MiB), the cones (748 MiB) and the revision store (611 MiB), this is the largest
item left. Filed as ENG-91 (over the ~100 MiB line).
