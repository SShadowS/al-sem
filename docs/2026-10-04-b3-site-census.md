# B3 Phase A, task 1 — L3 / program call-site join census

Tool: `aldump --b3 <workspace>` (`src/engine/l3/program_calls.rs`, `site_census`).
Program context: `BuildProfile { dependency_bodies: Summary }` (the profile
`fresh_coverage` uses; the resolver resolves workspace files only).

## Join

- Site key: `(unit, start.line, start.col, end.line, end.col)`. L3 unit =
  `source_unit_id` minus `"ws:"`; program unit = `CanonicalSpan::unit`. Both sides
  use 0-based rows and **byte** columns of the same IR node (the `Call` expression),
  so there is no column conversion. The non-ASCII test proves this.
- Then: callee fingerprint (`callee_fp(PCallSite.callee_text)` vs
  `SiteId.callee_fingerprint`), then caller (L3 routine declaration anchor vs
  `DeclSurface::get_with_path(edge.from)` → `(virtual_path, origin.start)`).

## Reasons

| Reason | Meaning |
|---|---|
| `matched` | L3 call site, program call edge at the same span, same callee fp, same caller |
| `no_program_site` | L3 call site, no program edge at that span |
| `shape_mismatch` | L3 call site, program sees a record op or `Commit` there |
| `callee_fp_mismatch` / `caller_mismatch` | span pairs, callee or caller does not |
| `program_only_site` | program call/run edge, nothing in L3 at that span |
| `op_shape_mismatch` | L3 record op, program call edge (bare implicit-`Rec` ops) |
| `operation_site` | program record op that cannot fire a trigger, or `Commit` (expected) |
| `implicit_trigger_matched` / `_unmatched` | program trigger-capable record op with / without an `L3RecordOperation` at that span |
| `l3_op_no_program_site_trigger` | L3 trigger-capable record op (Insert/Modify/Delete/Validate/Rename), no program edge at its span |
| `l3_op_no_program_site_other` | any other L3 operation site (non-trigger record op, lock, `Commit`), no program edge at its span |
| `duplicate_program_span` / `duplicate_l3_span` | a second site on one side with the same span (sanity counters) |

L3 totals: `l3_record_operations`, `l3_operation_sites` (record ops + locks + commits;
`error-call` sites are also call sites and are counted there).

## Results

| Corpus | L3 call sites | program sites | matched | no_program | program_only | fp | caller | shape | op_shape | operation | it_matched | it_unmatched |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| CDO (`DO-cdo-baseline/Cloud`, `bc3ccb18`) | 15,529 | 20,707 | 15,529 | 0 | 0 | 0 | 0 | 0 | 571 | 4,003 | 604 | 0 |
| DO (`DO/Cloud`, also `bc3ccb18`; `Cloud/AppSourceCop.json` dirty in that checkout) | 15,529 | 20,707 | 15,529 | 0 | 0 | 0 | 0 | 0 | 571 | 4,003 | 604 | 0 |
| r0-corpus, 206 dirs with `app.json` (sum) | 560 | 1,168 | 560 | 0 | 0 | 0 | 0 | 0 | 3 | 427 | 178 | 0 |

L3 operation sites with no program edge:

| Corpus | l3_record_operations | l3_operation_sites | l3_op_no_program_site_trigger | l3_op_no_program_site_other | duplicate_l3_span |
|---|---:|---:|---:|---:|---:|
| CDO | 5,120 | 5,178 | 0 | 0 | 0 |
| DO | 5,120 | 5,178 | 0 | 0 | 0 |
| r0-corpus (sum) | 542 | 608 | 0 | 0 | 0 |

Every L3 operation site has a program edge at its span. On CDO the 5,178 split exactly
into 4,003 `operation_site` + 604 `implicit_trigger_matched` + 571 `op_shape_mismatch`.
So the only L3 record ops without a matched program trigger edge are the 571
`op_shape_mismatch` sites (193 of them trigger-capable), below.

Every L3 call site on CDO pairs exactly: 0 of 15,529 unmatched. DO's checkout sits at
the same commit as the CDO baseline, so its numbers are identical.

## What does not pair: bare implicit-`Rec` record ops

All 571 CDO `op_shape_mismatch` sites are bare record ops with an implicit `Rec`
receiver (`Modify()`, `CalcFields(..)`, `TestField(..)`) in tables (445), pages (124)
and table extensions (2), across 76 files. L3 makes them record ops. The program engine
makes them bare calls — a documented approximation in
`src/program/resolve/extract.rs`'s module doc. 193 of them are trigger-capable
(`Insert`/`Modify`/`Delete`/`Validate`; 144 in tables, 49 in pages).

This is not a call-site join miss: no L3 call site is involved. It matters for the
adapter: if it takes implicit-trigger edges from the program engine, these 193 ops get
none where L3 today may produce one. The fix belongs in the program engine (classify
bare implicit-`Rec` record ops as `RecordOp`), not in the join.

## Known structural differences, pinned by tests

- **BOM**: L3 strips a UTF-8 BOM, the program engine does not, so a call on line 0 of a
  BOM file is `no_program_site` + `program_only_site`. Later lines pair. None on CDO.
- **Nested `app.json`**: L3 discovery stops at it, the program engine walks the folder,
  so its sites are `program_only_site`. None on CDO.

## Adapter counts (task 3)

Since task 3, `aldump --b3` runs the adapter (`resolved_calls_from_program`,
`adapter_census_for_workspace`) and prints its counts after the join counts. CDO
(`DO-cdo-baseline/Cloud`, `bc3ccb18`, release-fast, `ALSEM_NO_PREFLIGHT_CACHE=1`):

| Counter | CDO | Meaning |
|---|---:|---|
| `adapter_program_sites` | 15,529 | call sites whose edges came from the program engine |
| `adapter_l3_fallback_sites` | 0 | call sites answered by L3's own resolver |
| `adapter_callee_outside_l3` | 0 | matched sites whose workspace callee has no L3 routine (fall back) |
| `adapter_program_trigger_ops` | 604 | record ops whose trigger edges came from the program engine |
| `adapter_l3_trigger_ops` | 193 | trigger-capable ops with no program trigger edge: kept on L3 |
| `adapter_l3_trigger_edges` | 78 | edges L3 gave those 193 ops |
| `adapter_trigger_routes_filtered` | 1,005 | program trigger routes dropped by the site rules (`RunTrigger = false`, other field's `OnValidate`) |
| `adapter_routes_dropped` | 84 | interface/trigger routes into dependencies or unresolved |
| `adapter_trigger_edges_beyond_l3` | 3 | trigger edges L3 would not give the op (TableExtension triggers) |
| `adapter_trigger_edges_beyond_l3_rename` | 0 | the `Rename` part (structurally 0: neither engine makes `Rename` a record op) |
| `adapter_trigger_edges_l3_only` | 0 | matched ops where L3's own trigger edge is missing from the adapter's |
| `adapter_external_record_receiver` | 67 | `ExternalTarget` on a record receiver — L3 says `Unknown(RecordTableProcedure)` here, so the confidence cap changes |
| `adapter_external_object_receiver` | 640 | `ExternalTarget` on an object receiver — L3 says `ExternalTarget` too |
| `adapter_external_other` | 56 | `ExternalTarget` on a bare call or other receiver |
| `adapter_external_member_decline` | 0 | member declines on a non-workspace receiver mapped to `ExternalTarget` |
| `adapter_multi_route_sites` / `adapter_empty_route_sites` | 0 / 0 | |

r0-corpus (206 dirs, from the parity test): 560 call sites and 178 trigger ops from the
program engine, 0 call-site fallbacks, 3 trigger ops kept on L3.
