# OUTSTANDING stale-claim pass (TLS-45)

Every open item in `docs/OUTSTANDING.md`'s backlog and Parked sections was checked against
master `67d75bea`. Two read-only agents (stale-doc-hunter) did the reading; I checked every
claim that closes an item, changes its text or raises a defect in the source:
- `junction_guards` (`param_guard.rs:209`);
- the dependency publisher's `isolated` set from `publisher_isolated` (`events.rs:662-668`);
- `b3_diff` / `implicit_trigger_edge_for_op` gone;
- `first_resolved_edge_per_callsite` at `detector_context.rs:2500`;
- `SourceRoot.content_hash` hashing only the texts (`snapshot/provider.rs:57-62`);
- the implicit-conversion test (`arg_dispatch.rs:3092`);
- ENG-26's protected vars (`full.rs:1716-1724`);
- #83's `ir_object_key` (`ir_walk.rs:3865`).

Nothing was built or run, so a count that needs CDO or Base App (8020) is `undecided`.
Skipped: the engine-limits item (TLS-44 audited it) and the identifier-fold item (TLS-43, in
review).

Overall: `src/engine/l2` and `src/engine/l3` no longer exist (engine-switch S9). Every path
into them is moved or false. The `.superpowers/sdd/*` review files several items cite are not
in the tree.

## Verdicts

| line (title) | verdict | what changed |
|---|---|---|
| 73 False-positive causes exposed by S8.6 | keep | the blank-field rule is owned by ENG-13 |
| 131 d8 after S8 gap 3 | keep | `pending_writes` lives in `src/engine/l5/pending_writes.rs`; still may-commit only; the 9 findings are undecided |
| 151 Constant-argument guards | keep, text out of date | conjuncts are DONE (`junction_guards`, `param_guard.rs:209`); Integer, Option and Enum literal pruning exists (`literal_values.rs`, 5d41cd82); recursive SCCs get per-root literal reach; open: `case P of` on a Boolean, data state |
| 183 Detector policy for dependency routes | keep, text out of date | `first_resolved_edge_per_callsite` is in `engine::l5::detector_context` (:2500); "decide with S8" is a stale wake |
| 205 B3 harness L3 comparisons | **close (obsolete)** | `b3_diff` and `implicit_trigger_edge_for_op` are gone with L3; a leftover `duplicate_l3_span` counter stays in `SiteCensus` (`program_calls.rs`) |
| 212 Event inventory follow-ups | keep, text out of date | `isolated` IS set for dependency publishers now (`events.rs:662-668`); L3's `build_event_graph` is gone; `EventEdge` (`program/model/events.rs:84`) has `conditional` / `mixed_readings`; the d38 bullet is still true |
| 231 B3 Phase A follow-ups | keep, text out of date | `calls_for(..).into_owned()` no longer exists; the `--l3-*` tail is gone (S9.6); still open: `aldump` does not print `abi_ingest_errors` |
| 334 Engine memory/speed Track B | keep, text out of date | the comparator item is fixed by TLS-27 (`registry.rs:841-885`; `finding.rs:988/1107` call it); B3 (single substrate) is done (S6-S9) and the "L3 load-bearing" sentence is obsolete; B1 has no sequencing with the `to_lowercase` census (TLS-42 recommends no migration); mimalloc now lives in `src/release_alloc.rs`; `snapshot.rs:154-265` is now `src/snapshot/`; the "Current 8020 state" and first lever list are superseded by the item's own later text |
| 680 Multi-root follow-ups | keep | unchanged (`server.rs:1352`, `watcher.rs:19`) |
| 784 L-3 globals sharing | keep, text out of date | mechanism moved to `program/body/ir_walk.rs:3607` and `program/model/workspace.rs:2099/2190`; the wake "next L3-substrate pass" no longer exists |
| 860 FindingConfidence carries ids | keep, text out of date | `UncertaintyTable` is deleted; a d1 confidence bucket would go in `ALSEM_RETAINED_CENSUS` |
| 882 Residual duplicate-id groups | keep, text out of date | "0 on DO" holds for the workspace; the CDO cross-app model has 2 preproc-arm groups in Base App `PageManagement` (CHANGELOG 2.x); `compute_routine_id` is in `program/body/scope.rs:161` |
| 1038 d55 rootCauseKey | keep | code at `d55.rs:72/75` |
| 1054 Preflight shared parse | **close (obsolete)** | L3 is gone; `analyze` parses once through the program context |
| 1069 FreshCoverage reporting | keep, text out of date | ABI-error and missing-dependency reporting exist (`dependency_ledger`, FW2, test `abi_ingest.rs:1765`); open: the serde-default-empty exemption (`full.rs:3579`) |
| 1074 Number-less object identity | **close (fixed by #83, 2.0.2)** | ids take `ir_object_key`; residual: an empty object name, a table number that does not parse |
| 1084 ProvenAbsent | keep | undecided (needs the CDO call-graph stats) |
| 1085 Implicit conversions | **close (built)** | implemented with `implicit_conversions_follow_the_compiler`; `ambiguousResolved` is 0 for CDO primary but up to 459 in dependency bodies (`program_resolve_harness`), so the wake as written is already met |
| 1086 Full ParseStatus gate | keep | unchanged |
| 1087 Protected `Variables[]` | **close (fixed by ENG-26)** | residual is ENG-63 |
| 1088 Preproc-symbol fidelity | keep | owned by ENG-53 |
| 1089 Sender param-type drift | keep | undecided |

## Defects with no coord task (for the orchestrator to file)

1. **`SourceRoot.content_hash` ignores paths and file boundaries** (`src/snapshot/provider.rs:57-62`):
   it hashes the concatenated texts only, so a rename, or the same bytes split differently
   across files, gives the same hash. Read by `program/dep_cache.rs:53-72` (dependency source
   cache) and emitted at `engine/impact/jobs.rs:376`. Fix: fold each file's path and a
   length prefix in; test two file sets with the same bytes split differently.
2. **d55: several publish sites in one loop share one fingerprint** (`d55.rs:75`, the finding
   id at :72 carries the call site): baselining one silences the others (DO: 2 groups / 3
   excess, measured 2026-07-25). Not covered by ENG-28 (a different fingerprint cause).
3. **d38 cannot see Obsolete on dependency event publishers**: `EventSymbol`
   (`program/model/events.rs:46-65`) has no Obsolete field, so d38 skips them as
   `noPublisherRoutine` (93 on CDO when recorded; undecided now).
4. **Doc links to the deleted `d1_cohort::UncertaintyTable`**: `engine/l5/confidence.rs:35`,
   `engine/l4/summary.rs:84` and `:122` (doc-only).
5. Lower priority, owner's call: `aldump` does not print `abi_ingest_errors` (done by
   TLS-47; a package the loader drops never reaches that field, engine limit L37); d35 Group 2
   (`GuidedExperienceImpl` Commit at :115) should be re-measured now that conjunct guards
   exist; the symbol-only exemption cannot tell an empty `SymbolReference` from a
   serde-default one (`full.rs:3579`; no population).
