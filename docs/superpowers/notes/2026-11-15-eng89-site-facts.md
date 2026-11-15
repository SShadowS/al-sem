# ENG-89: what the 385 MiB of dependency site facts is

Measured 2026-11-15 on server1 (kraken tentacle), master `4dcf4b44`, pinned CDO baseline
(`/work/src/cdo-baseline/Cloud`, `bc3ccb18`), under `scripts/cdo-slot`. Tool:
`tools/census-probe resident` (counting allocator, requested sizes), which now splits
`session.dep_resolution.site_facts` by part; the run is in `tools/census-probe/runs-eng89/`.
Only the dependency side was measured; the workspace's own site facts (in the model build) use
the same type and would gain in proportion.

## Headline

**The bytes are the map's layout, not facts.** `site_facts` is a
`HashMap<ObligationId, SiteFacts>` with 318,982 entries in a table of capacity 458,752. Each
slot is 128 B of key + 592 B of `SiteFacts` + 1 control byte, so the table alone is
**315.4 MiB** of the measured 384.9 MiB. All per-field heap together is about 6.5 MiB.
Almost every field is empty on almost every site.

## The split

| Part | MiB |
|---|---:|
| table inline: capacity 458,752 × (128 + 592 + 1) B | 315.4 |
| keys' file path (`span.unit`, one `String` per key, 7,003 distinct files) | 16.1 |
| fields' heap (all of them) | 6.5 |
| remainder (heap not walked: receiver types, ids) | 47.0 |
| **measured drop** | **384.9** |

Every key is `ObligationId::CallSite`. A table at 70% load is hashbrown's normal state after
doubling (it grows at 87.5%), so `shrink_to_fit` would not help.

Fields by use (318,982 sites):

| Field | Sites using it | Heap MiB |
|---|---:|---:|
| `bare_scope_complete` (bool) | 152,861 (48%) | 0 |
| `receiver` | 141,165 (44%) | 4.5 |
| `unqualified_record_op` (bool) | 9,544 | 0 |
| `validate_field` | 8,146 | 0.1 |
| `start_method` | 5,212 | 0.0 |
| `run_trigger_excluded` | 3,838 | 1.1 |
| `table_event_raise` | 3,292 | 0 |
| `build_context` (22 distinct contexts) | 2,729 | 0.1 |
| `interface` | 1,198 | 0.0 |
| `run_target` | 928 | 0 |
| `dataset_run` | 649 | 0.5 |
| `open` | 579 | 0.1 |
| `request_page_may_cancel` | 376 | 0 |
| `record_table_unresolved` | 208 | (not walked) |
| `argument_codeunits` | 93 | 0.0 |
| `field_ref_validate` | 74 | (not walked) |
| `interface_closed_over` | 69 | 0.0 |
| `interface_holder` | 48 | 0.0 |
| `call_result_record`, `inherited_protected` | 34 each | 0 |
| `interface_unmapped_enums`, `record_op_field_named` | 0 | 0 |

## Shrink proposals

1. **A hot part and a boxed cold part** (the big one). Keep `receiver` and the bools inline and
   move the 19 rare fields into one `Option<Box<RareSiteFacts>>` (under 7% of sites have any).
   The record goes from 592 B to roughly `size_of::<Option<ReceiverFact>>()` + a few bools + 8 B
   (to measure; ~100 B expected). In today's table that alone is about 458,752 × 490 B ≈
   **215 MiB**, plus a rare box (~600 B) for under ~22,000 sites (~13 MiB back).
2. **No second copy of the key.** Every retained edge already carries its `ObligationId`, and the
   readers look facts up by it (`program_calls`, `compiler_golden`, `prove_slice`, `aldump`),
   or iterate. Storing the facts in a `Vec` indexed from the edge (a `u32` on
   `ClassifiedEdge`), or in a `Vec` sorted by key, drops the 128 B key and the table's 30% slack:
   with proposal 1, about 318,982 × ~100 B ≈ 30 MiB in place of the table's ~100 MiB. That is
   **another ~85 MiB** (keys 458,752 × 128 B = 56 MiB, the empty 30% of the slots ~13 MiB, the
   keys' file paths 16 MiB; a `u32` per edge costs 1.7 MiB back). All reads go through
   `SiteFactsMap`/`SiteFactsRef` (`src/program/resolved.rs`), so the change stays behind them.
3. **Not worth it on its own:** interning `build_context` (22 distinct contexts, 0.1 MiB) and the
   field strings (6.5 MiB in all).

Together, 1 and 2 put the 385 MiB at roughly 100 MiB (the values ~30, the rare boxes ~13, the
field heap and the unwalked remainder ~50): **about 285 MiB** off the 5.5 GiB resident, the
largest single item left on the census after the cones and the revision store. The estimates
assume a ~100 B hot record; `size_of::<Option<ReceiverFact>>()` decides it, so measure that
first. Filed as its own task (over the ~100 MiB line).
