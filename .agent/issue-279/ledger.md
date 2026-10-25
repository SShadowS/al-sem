# #279 ledger

## Claim

```json
{"run_id":"20261025-121831-b805ac","issue":279,"attempt":1,"session":"claude-code-session:57653578-e111-4b15-8684-017371f816fa","branch":"issue/279-agent-discovery-policy-detector-a1","worktree":"/work/al-sem-issue-279-a1","body_hash":"64ed68297ac710fd38151866f9a2454f2007d3f005be5d1a4c84c78740cef4ae","reconciled":[]}
```

Base: `3f5030f9d0ba6c3f9e073b850b7de5bc1de2f801` (make-worktree, from origin/master). Body hash: `64ed68297ac710fd38151866f9a2454f2007d3f005be5d1a4c84c78740cef4ae`. Reviewers: gpt-6.1-sol (astra slot) + local Opus (flash slot), attest with `--substitute astra=sol --substitute flash=opus` (operator). Revision 1 of Classification / Probes / Design is kept in the run dir (`ledger-rev1-snapshot.md`).

## Classification

**Bounded.** Revision 2. One mechanical swap at a closed list of sites: each reader of "this routine's object" moves from a last-wins id lookup to the existing, tested `DeclaringObjects::of` (#241); the `[HandlerFunctions]` lookup changes its key the same way. Nothing new in the data model, no id or schema change. CDO output byte-identical before and after at both scopes and with the opt-in detectors. Not a spike (reproducer confirmed in both arm orders). Not architectural: the program graph's deliberate merge of `#if` arms into one node per id is out of scope (Discoveries 1 and 3).

## Assumption probes

Evidence: `/work/bench/279/rev2/` (prototype `/work/bench/279/proto`, 23 files +263/−135, diff `rev2/proto.diff` — the checked-out tree equals it byte for byte, so every break is reverted; fixtures `rev2/fixA` = interesting arm first, `rev2/fixB` = reversed). Rev-1 evidence in `/work/bench/279/`.

| probe | result | evidence |
|---|---|---|
| contract of `DeclaringObjects::of` | one row → that row; several → the row whose anchor contains the routine's start in the same file (innermost wins); no match → the last row; the no-anchor fallback is pinned by U1 in `effect_query_cli.rs:1532+` | `src/program/model/declaring.rs` |
| is the last-row fallback reached for split ids? | not in well-formed source: only workspace source makes a multi-row id; workspace rows always carry an anchor (`workspace.rs:104-107` "native assembly only"; set at `:1145-1148`) and each arm's routines lie inside their own arm's anchor; ABI rows have `source_anchor: None` but one row per id (`abi_rows.rs:64`). Having an anchor is not the same as having a *matching containing* anchor: `of` also falls back when an anchored id's rows contain no routine start (`declaring.rs:53-64`; U1 shows this with anchored rows, `effect_query_cli.rs:1559-1583`). That case is not produced by the current assembly and stays a documented limitation | source reads |
| base reproducer, both arm orders | confirmed: fixA (P deleting, first arm) policy `bar-deletes` matches P (wrong); fixB (P deleting, second and last arm) `foo-deletes` matches P at `:13` — correct by accident, because the last-wins map happens to pick P's own arm — while `install-inserts` matches both arms (wrong) | `rev2/base-fixA/summary.txt`, `rev2/base-fixB/summary.txt` |
| base bug: d14 | fixA no finding on `DeadLive`; fixB false positive on `DeadInTest` (rootCause `DeadInTest on Live`) | same |
| base bug: Test subtype | `excludedTestCode` 0 for Helper (fixA), 1 for Main (fixB) | same |
| base bug: implicit `Rec` | `OnOpenPage` and `Wipe` resolve to the same table (`Other Rec` in fixA, `Probe Rec` in fixB) | same |
| base bug: d60 and d57 (old any-arm check) | d60 fires on `Rewrite` (normal arm), d57 on `PlainSub` (non-SingleInstance arm), both orders | same |
| base bug: d39 `page_may_modify` | fixA misses `OnAction` on the read-only page; fixB false positive on `Touch` (editable page) | same |
| base bug: HTML labels | fixA labels the publisher `Pub B` and the subscribers `SI Plain :: KeepSub`, `SI Plain :: PlainSub` | HTML event-graph node titles |
| base bug: `[HandlerFunctions]` | `Prod` shows covered through the other arm's `H` in both orders (phantom coverage edge) | `digest-Prod.json` |
| no CDO object is split this way | CDO's one `#if` (`CDOEMail.Codeunit.al:241-252`) is inside a routine body | grep |
| the #160-owned files need no edit | `digest.rs` relies on the swap in `return_summary.rs`; `ordering_engine.rs` has no object lookup; `fingerprint_query.rs` takes its owner from `OwnerIndex` (#241) | greps |
| SmallVec slot (O8) is trivial | **No**: `SmallVec<[&'a _; 1]>` is invariant in `'a`; the build failed with lifetime errors at `policy_engine.rs:229`, `predicate_fields.rs:324`, `run.rs:847/1031`, `d1_dataflow.rs:3160`; kept `Vec` | observed in the design author's first rev-2 build attempt; that failing log was overwritten (`rev2/build-after.log` holds the later successful build), so this is reported, not retained |

Inventory of id-keyed readers (revised):

| site | scope | verdict |
|---|---|---|
| policy `object_of` (`predicate_fields.rs:411`, built at `policy_engine.rs:141`) | routine | swap; pinned by R1 |
| `DetectorContext.objects_by_id`: d13, d17, d14 (two reads), d46, d2, `mod.rs` `page_may_modify` | routine | swap; the field is replaced by `declaring`; pinned by R3 (both d14 reads) and R8; d2, d13, d17, d46 unpinned (Not delivered) |
| d50 `is_explicit_commit_proven_effective` | routine | swap; pinned by R11 |
| d57 SingleInstance id-set, d60 lifecycle id-set | routine membership | swap; pinned by R7, R6; intentionally changes the old any-arm meaning |
| `root_classification` `classify_roots`, `classification_per_id` | routine | swap; `classify_roots` pinned by R2 and R13a; the per-id selection in `classification_per_id` (reached only when one id has two different copies, `root_classification.rs:345-363`) pinned by R13b |
| `test_coverage` Test-subtype check (`:476`) | routine | swap; pinned by R4 |
| `test_coverage` `[HandlerFunctions]` lookup keyed by `(object_id, name)` (`:419-424, 464-468`) | routine | swap; the key becomes the declaring row's address; pinned by R10 |
| `return_summary::compute_return_summaries` | routine | swap; pinned by R12 |
| `workspace.rs resolve` (implicit `Rec`) | routine | swap; pinned by R5 |
| `format_html` `routine_label`, `pub_label` with its object fallback | routine / event | swap; pinned by R9 |
| `FingerprintIndex::object` | object | renamed `object_row_by_id`, doc "object-scoped only, never a routine's object"; its two callers are object-scoped (`gate/projection.rs:84` object-anchored finding, `gate/run.rs:558` role); exempt |
| `closed_world_temp.rs:122-127` `routines_by_object` | sibling grouping | exempt: takes both arms' siblings (a superset), conservative |
| `l5/snapshot.rs:1678` `derive_identity_table` | object display of a target | exempt: names a target object, not a routine's owner |
| `l5/fingerprint.rs` (type/key), `event_flow.rs:827`, `registry.rs:522`, `abi_rows.rs:267` | app / type / role | exempt: the id embeds guid and type, so both arms agree |
| `symbol_table` lookups by type+name/number, `combined_graph.rs:627`, `lsp/custom.rs:385` | target lookup | exempt (object-scoped) |
| program graph `graph.objects` (resolve, impact_graph, graphify, integration_report) | merged node | exempt by design (`build.rs dedup_objects`); Discoveries 1 and 3 |

## Design

Revision 2.

**Goal.** Every reader of "this routine's object" — and the `[HandlerFunctions]` lookup — gets the routine's own declaring row, so an `#if`-split object no longer gives one arm's name, subtype, SourceTable, InherentCommitBehavior or SingleInstance to the other arm's routines.

**Non-goals.** The program graph's merged node per id stays (Discoveries 1, 3). No fingerprint or id changes. A routine id shared by both arms (same signature) stays one id; readers that look a routine up by id still see one row (Not delivered). No edits to the #160-owned files.

**Changes** (in `rev2/proto.diff` unless marked *to do*).
1. `program/model/declaring.rs`: `census_tally()`; a comment records why `Vec`, not `SmallVec`.
2. `l5/fingerprint.rs`: `object` renamed `object_row_by_id` with the object-scoped-only doc; callers `gate/projection.rs:84`, `gate/run.rs:558` updated.
3. `l5/detector_context.rs`: `objects_by_id` replaced by `declaring: DeclaringObjects<'v>`; census bucket `context.declaring`; module doc line 5 updated.
4. Detectors: `d13`, `d17`, `d14` (both reads), `d46`, `d2`, `mod.rs` `page_may_modify` → `ctx.declaring.of(r)`. `d57`, `d60`: the predicate on `ctx.declaring.of(r)`, early exit `ws.objects.iter().any(pred)`, a comment stating the fallback exception. `d50`: parameter `&DeclaringObjects`; 14 unit-test call sites and the stale comments at `:640`, `:710` updated.
5. `gate/policy/predicate_fields.rs`, `policy_engine.rs`: `FieldIndexes.declaring`.
6. `root_classification.rs`: the private `ast_classification` takes `&DeclaringObjects`; the public `classify_roots(&ModelEntities)` and `classification_per_id(roots, routines, objects)` keep their signatures and build it inside.
7. `l5/test_coverage.rs`: the Test-subtype check reads `declaring.of(r)`; the `[HandlerFunctions]` key is `(object_id, declaring row address)` via a nested `fn owner`.
8. `return_summary.rs`: ICB from `declaring.of(r)`.
9. `program/model/workspace.rs resolve`: `declaring.of(routine)`.
10. `gate/format_html.rs`: `Maps.declaring`; `routine_label` uses it; `pub_label` goes publisher routine → `declaring.of`, else the object row.
11. `tests/cli/cli_batch_census.rs`: `context.declaring`. `tests/r4/r4_differential.rs:1177,1198`: comments updated.
12. New lib tests: `d50::tests::cap4_reads_the_declaring_arms_icb`, `return_summary::tests::bulk_icb_merge_reads_the_declaring_arm`, `root_classification::tests::split_arms_classify_against_their_own_row`.
13. *To do at implementation:* the CLI acceptance tests R1–R10 (tempdir fixtures in the `cli_object_display.rs` style, from `rev2/fixA` and `rev2/fixB`) and the CHANGELOG `Fixed` entry.

**Acceptance matrix.** Every CLI row runs through `CARGO_BIN_EXE_alsem` on both arm orders (fixA, fixB) and asserts the positive and the other arm's negative. "Breaks" = the global lookup breaks B1 (last row) and B2 (first row) unless a row names another; each catches every row in at least one order (outcomes recorded in `rev2/break-*.txt`). At implementation each test still gets its own single-site discrimination proof.

| # | requirement | test (production assembly) | break that must fail it (observed) |
|---|---|---|---|
| R1 | policy `object.name` | `policy check`: `foo-deletes` matches P (Foo, deletes) only; `bar-deletes` matches nothing; both orders (P first in fixA, second in fixB) | B1 fixA → `bar-deletes` on P; B2 fixB → `bar-deletes` on P at `:13` (the S1 re-run: first-wins now fails) |
| R2 | `root.kinds` from own arm | exactly one `install-inserts`, on `OnInstallAppPerCompany`; none on `Plain`; both orders | B1 fixA → no match; B1 fixB → extra on Plain `:3`; B2 fixA → extra on Plain `:15` |
| R3 | d14, both reads | exactly one d14: `DeadLive`, objectName `Live`, rootCause starts `DeadLive on Live is not called`; none on `DeadInTest` | B1 fixA → none; B2 fixA → extra `DeadInTest`; B6 (only `d14.rs:195` to last row) → rootCause `DeadLive on Live Test …` |
| R4 | Test subtype, both arms | digest Helper: `excludedTestCode` 1, `changedRoutines` 0; Main: 0 and 1 | B1 fixA → Helper 0/1; B2 fixA → Main 1/0; mirror in fixB |
| R5 | implicit Rec through own SourceTable | `query effects OnOpenPage` = `["Probe Rec"]`; `Wipe` = `["Other Rec"]` | B1 fixA → OnOpenPage `Other Rec`; B2 fixA → Wipe `Probe Rec` |
| R6 | d60 (Upgrade arm has a real loop) | exactly one d60, on `UpgradeRows [Probe Upgrade]`; none on `Rewrite` | B1 fixA → none; B1 fixB → extra `Rewrite`; B2 mirror; B3 (any arm) → extra `Rewrite` both orders |
| R7 | d57 SingleInstance membership | exactly one d57, on `KeepSub [SI Keep]`; none on `PlainSub` | B1/B2 → extra `PlainSub` in the order where the SI arm is the chosen row; B3 → extra `PlainSub` both orders |
| R8 | d39 `page_may_modify` | exactly one d39, on `OnAction [Probe RO]`; none on `Touch [Probe Edit]` | B1 fixA → `OnAction` lost; B1 fixB → `Touch` reported; B2 mirror |
| R9 | HTML labels | `analyze --format html` event-graph node titles contain `Pub A`, never `Pub B`, plus `SI Keep :: KeepSub`, `SI Plain :: PlainSub`, and the table-event publisher `Probe Rec` (fallback); subscriber nodes asserted on fixA (fixB carries Discovery 3's unresolved node) | B1 fixA → `Pub B`, `SI Plain :: KeepSub`; B2 fixB → `Pub B`; B5 (object fallback removed) → node `27927927-…/Table/50190` instead of `Probe Rec` |
| R10 | `[HandlerFunctions]` only from own arm | `digest --routine Prod --tests`: `Prod` uncovered with `no-path`, both orders | B4 (key back to `object_id` only) → `Prod` covered both orders; B1/B2 also fail it |
| R11 | d50 cap 4 | lib `cap4_reads_the_declaring_arms_icb`: two rows under one id (set by assignment), ICB `ignore` on lines 0-10, none on 11-20; routine at line 2 → false, at 12 → true | B1, B2 fail (`rev2/break-B1.txt`, `break-B2.txt`) |
| R12 | `compute_return_summaries` | lib `bulk_icb_merge_reads_the_declaring_arm`: routine a → `ignore`, b → `normal` | B1, B2 fail |
| R13a | shared-signature id, one root arm | lib `split_arms_classify_against_their_own_row`: two rows of one routine id, `local` procedure, Install arm and normal arm, both orders; `classify_roots` gives exactly one copy `install-codeunit`; `classification_per_id` gives that copy even when the final row is the non-root one | B1, B2 fail |
| R13b | shared-signature id, two root arms of different kinds plus a final non-root arm | lib test (to write at implementation): three anchored rows sharing one routine id, in the order **Upgrade, Install, normal**, each declaring the same local, non-Test procedure, so `classify_roots` keeps two different copies; `classification_per_id` must return **Install** (the last classified row) | revert only the per-id declaring lookup in `classification_per_id` (lines 352-355) to the old last-object lookup → every recomputation sees the normal row and yields no classification, so the initially retained Upgrade copy survives → the test fails (a two-row Install/Upgrade case would not discriminate: the old last-wins lookup picks the same last-arm copy). Second break: a first-row pick in the per-id lookup → Upgrade is returned → the test fails. Both breaks are run and recorded |
| R14 | census name | `cli_batch_census` with `PER_CONTEXT` = `context.declaring` | — |
| R15 | nothing else moves | `check-goldens` + CDO byte identity at both scopes and with the opt-in detectors | — |

**False-positive and failure shapes.**
- (a) Shared routine id (same signature in both arms): each row classifies against its own arm, so `classify_roots` can keep two distinct copies for one id (class K); the extra copy shows wherever the full list is read (snapshot root slots `l5/snapshot.rs:3001`, the R4-F projection `root_classification.rs:761`, `switch_dump.rs:100`, `snapshot_full.rs:232,255`). Per-id readers go through `classification_per_id`, which returns the copy of the **last classified (owning) row**; when the final row is not a root, an earlier row's copy survives (R13). The per-id result does not stand for both arms. No golden fixture and no CDO object is split, so nothing moves today.
- (b) Fallback: for a multi-row id with no matching containing anchor, `of` returns the last row — the old answer at every map-based site, but not the old d57/d60 any-row meaning (documented in their comments; the d57 comment is to be corrected at implementation from "unanchored multi-row ids" to "multi-row ids whose anchors contain no routine start"). Not reached in well-formed source (each arm's routines lie inside their own anchor) and not for ABI rows (one per id); an anchored id whose rows contain no routine start would still fall back — a documented limitation, not produced by the current assembly; U1 pins the fallback order.
- (c) d57 clearing evidence: `cleared_by_object` stays keyed by id; a clear in one SingleInstance arm suppresses the other SingleInstance arm — hides findings, never adds false ones (Discovery 2).
- (d) Guard scope: only readers through `DetectorContext` are guarded structurally (d2, d13, d14, d17, d46, d57, d60, `page_may_modify`), and only while nobody calls `fingerprint_index.object_row_by_id` for a routine — a doc and naming rule, not a type rule. `format_html` keeps `Maps.objects` for the fallback. d50 and `return_summary` build their own `DeclaringObjects`, guarded by R11 and R12.
- (e) d13 and d17 compare `app_guid`, shared by both arms (the id contains it) — no behaviour change.

**Measurement (rev 2, done).** Fixture after the change: identical results in both orders, all R1–R10 correct (`rev2/after-fixA/summary.txt`, `rev2/after-fixB/summary.txt`); base wrong in both orders. Breaks B1, B2, B3, B456 as in the matrix (`rev2/break-*.txt`, outputs in `rev2/break-*/`); after reverting, `git diff` equals `rev2/proto.diff`. `scripts/check-goldens` 1,251 run, 1,251 passed, 4 skipped, exit 0. Lib tests of the touched modules: 307 passed, incl. the three new ones. CDO (the pinned CDO baseline @ bc3ccb18eb68, 0 dirty, release-fast): `--scope primary` 2,323 → 2,323, fingerprints 0 added / 0 removed / 2,316 kept; `--scope all` 30,618 → 30,618, 0 / 0 / 30,590; `fingerprint.json`, `analyze-primary.json`, `analyze-all.json` byte-identical; opt-in d17, d46, d50 at both scopes byte-identical. No finding moved. Wall times unusable (load average 9–88): no performance claim; one extra heap `Vec` per object id at each build site, so `context.declaring` census bytes exceed the old `context.objects_by_id`.

**Pre-stated fallbacks.**
- (a) A golden moves at implementation: triage with golden-diff-triager before any regen (no in-repo fixture is split, so a move means a bug in the swap).
- (b) A caller of the removed `DetectorContext.objects_by_id` turns up outside this list: swap it if routine-scoped; if object-scoped use `fingerprint_index.object_row_by_id`. Never bring back a routine-facing id map.
- (c) The census rename clashes with a #160 census expectation beyond `PER_CONTEXT`: keep the name `context.objects_by_id` with the new tally and record it.
- (d) A reviewer insists on SmallVec: it needs a covariant wrapper or a `RoutineIx`-style index slot over `&[ModelObject]` (as `projection::RowsOfId`); a separate change.
- (e) `ptr::from_ref` identity in `test_coverage` is rejected: key on the declaring row's anchor start `(source_unit_id, start_line, start_column)`.

**Not delivered.**
- Shared routine id, first arm's row: readers that look a routine up by id still see the last arm's answer (class P) — policy `root.kinds` (`policy_engine.rs:145`), the detector root classification (`detector_context.rs:1770`), and every detector starting from `ctx.routine_by_id` (e.g. d50 cap 4 at `d50.rs:137`, the d13/d17 caller lookups). For one policy row, `object.name` (own arm) and `root.kinds` (last owning row) can disagree.
- No separate pin for d2 (display), d13/d17 (no behaviour change, shape e), d46 (opt-in); they rely on the guard in shape (d).
- SmallVec slot: not trivial (invariance); kept `Vec`, cost noted.
- Program-graph owner names and name resolution: Discoveries 1 and 3.
- d57 per-id clearing evidence: Discovery 2.
- CHANGELOG entry and the CLI tests R1–R10: to do at implementation.

**Discoveries.** D1 program-graph owner names for second-arm routines. D2 d57 per-id clearing evidence. D3 a by-name reference to the second arm of a split object does not resolve (pre-existing; seen in fixB). JSON in `## Discoveries`.

## Timeline

- 12:2x claim #279 attempt 1 (operator-approved queue). Worktree at 3f5030f9. Probes + design + prototype by an opus design author (`/work/bench/279/`); classified bounded; CDO byte-identical. Design pasted.
- Spec panel r1 (spec_rounds 1 of 5): sol approve with fixes (S1–S4 important: R1 break non-discriminating, missing other-arm negatives, unguarded independent lookups, fallback vs any-arm membership; S5–S8 minor); Opus approve with fixes (O1 important: `FingerprintIndex::object` back door + d50/return_summary tests; O2 important: R6 vacuous, first-row vs last-row; O3–O8 minor). Register 16 entries, all `open`. Design revision 2 dispatched to the same author.
- Spec panel r2 (spec_rounds 2 of 5, diff-only): Opus approve (all 16 accepted; minor N1 = `classification_per_id` per-id branch unpinned); sol approve with fixes (re-raised S4/O5: anchored ≠ matching anchor, fallback "unreachable" overstated; new important: R13 never reaches the per-id branch; minor: fixB probe names Q not P, SmallVec failing log not retained). Conductor edits (revision 2.1): probe rows corrected; fallback qualified to well-formed source with the limitation kept and the d57 comment fix noted; R13 split into R13a + new R13b (two root arms of different kinds, per-id lookup break); inventory row updated.
- Spec panel r3 (spec_rounds 3 of 5, diff-only): both approve with fixes — R13b's two-row case cannot catch the old last-wins revert (S9/O9 re-raised); fixB probe misstated (S10). Revision 2.2: R13b uses three rows Upgrade, Install, normal (expect Install) with two breaks (last-object lookup; first-row pick); fixB probe corrected.
- Spec panel r4 (spec_rounds 4 of 5, diff-only): **both approve**, all 19 accepted → **converged**. Opus note for implementation: R13b should also assert that the first loop alone gives `upgrade-codeunit`, so Break 1 cannot turn green unnoticed if the kind names' sort order ever flips.
- Plan 2 tasks (plan_tasks charged 2); implementation dispatched (opus): acceptance tests R1–R13b, plan, implementation from the rev-2 prototype, proofs.
- Design revision 2 measured and pasted (both arm orders; global breaks B1/B2/B3 and single-site B4–B6 all fail their rows; `[HandlerFunctions]` swapped in scope; `FingerprintIndex::object` renamed object-scoped; three new lib tests; CDO byte-identical; new Discovery 3).
- Implementation (opus, 2026-10-25). Tests first: `tests/cli/cli_split_object_readers.rs` (R1–R10, tempdir fixture = `rev2/fixA` arms, both orders generated from one table), lib R11, R12, R13a, R13b; census rename R14. Red tree = those tests + a d50 interface skeleton (`is_explicit_commit_proven_effective(&DeclaringObjects)` with a temporary `skeleton_last_row` = old last-wins lookup), snapshot commit object `f9de31ea` (never a branch commit), built in a throwaway worktree. Paused for the coordinator's CDO bench before building; resumed.
- Red on base (`/work/bench/279/impl/red-lib.log`, `red-cli.log`): all 14 new tests FAIL, each for its stated reason, in the interesting-first order: R1 `foo-deletes` [] (P read as Bar); R2 `install-inserts` []; R3 no d14 (`DeadLive` read as `Live Test`); R4 Helper 0/1; R5 OnOpenPage `Other Rec`; R6 extra `Rewrite`; R7 extra `PlainSub`; R8 `OnAction` lost; R9 `Pub B`, `SI Plain :: KeepSub`; R10 `Prod` covered; R11 line-2 routine `true` (ICB of the last row); R12 `a` = `normal`; R13a `classify_roots` [] (both rows see the normal row); R13b fails its first precondition on base (`classify_roots` is also last-wins there); its own per-id site is shown by its two single-site breaks. R14 (`cli_batch_census`) PASSES on base: before the rename `context.objects_by_id` simply counts as a shared bucket; the row has no break (matrix "—").
- Green: the same 16 tests pass (`green-lib.log`, `green-cli.log`). One deviation from Design change 6: the private `ast_classification` takes the routine's resolved `&ModelObject` (callers do `declaring.of(r)`), not `&DeclaringObjects`, so the per-id site in `classification_per_id` can be reverted alone (R13b break 1). Plan: `docs/superpowers/plans/2026-10-25-issue-279-declaring-objects.md`.
- Single-site proofs: 19 breaks, each the only `PROOF-BREAK` in the diff (asserted before each run), each reverted with Edit; every one but R14's failed its test; after all reverts `git diff` has 0 `PROOF-BREAK` and every new test passes (`proofs/pass-lib.log`, `proofs/pass-cli.log`). "Last row" at sites with no object slice is emulated by asking `of` for a clone of the routine with its anchor file cleared (no anchor matches, so `of` returns the last row, the old id-map answer). R14's rename break stays green (stated limit, no pin).
- Conductor: task gate `ci-steps task` exit 0 on ba5a5f0c; pushed; draft PR #320. Final panel r1 (final_rounds 1 of 5): sol approve with fixes (FS1–FS3 minor), Opus approve with fixes (FO1–FO6 minor), code-review skill at high (FC1–FC10). FC1 refuted (Discovery 2), FC2 deferred (new Discovery 4), FO6 deferred (cosmetic). Fixes 49a952d3, 90ca96dd, 52bc5706, 1727e768 (the last removes a `#[allow(clippy::too_many_arguments)]` check-diff refused). Task gate exit 0.
- origin/master moved (#246, #319, #309): clean rebase onto b229d970 with `core.commentChar=';'`; range-diff shows only CHANGELOG context lines (no extra panel round needed); new SHAs 0e170540, 741b7a89, 045b51e4, 49b8a521, febc9a5a, 13e6ac51; register `fix_commit`/anchors remapped.
- Final panel r2 (final_rounds 2 of 5, diff-only): Opus approve, sol approve with fixes; all 19 final-1 ids accepted by both (incl. FC1 refutation, FC2/FO6 deferrals). New minors FR2a (static `OnceLock` TempDirs never dropped), FR2b (nextest doc), FR2c (eager index in `build_detector_artifacts`). Running branch gates stopped (code change pending). Fix 729d046e; proofs r3-*; task-affected 4,429 passed; CDO 729d046e vs 13e6ac51 byte-identical.
- Final panel r3 (final_rounds 3 of 5, diff-only): **both approve**, FR2a–c accepted. Register converged: 41 entries (19 spec, 19 final-1, 3 final-2), every one accepted by sol and Opus. Branch gates started on 729d046e.
- Branch gates on 729d046e: ci-steps-all exit 0, check-goldens-coverage exit 0, check-goldens exit 0 (tree stamp), **cdo-gate exit 1: not a test verdict** — `program_resolve_harness` was killed by SIGKILL (`signal: 9`) during `cdo_i264_closed_interface_site_census`; the container cgroup (`memory.max` 32 GiB) records `oom_kill 7`, i.e. memory pressure from concurrent sessions. Every other cdo-gate test passed (2,582 lib, harness members before the kill ok). Re-run requested in a quiet window. alsem-second confirmed its #111 gates (incl. cdo-gate) overlapped; with the box quiet, **cdo-gate re-run PASS** (exit 0, `logs/cdo-gate/20261025T160809-db2d33e4.log`), no new oom_kill.
- Note for the operator: about 116 temp dirs leaked by the earlier `OnceLock` test version remain in `/tmp` (`/tmp/.tmp*` with `app.json` guid `27927927-0000-0000-0000-000000000279`); removal was blocked by the tool's safety check, so it is left to the operator.

## Discrimination proofs

| test | mutation | fail output | pass output | commit |
|------|----------|-------------|-------------|--------|
| R1 `r1_policy_object_name_is_the_declaring_arm` | `predicate_fields.rs object_of` → last row | `foo-deletes in InterestingFirst` left [] right [(Split.Codeunit.al, 3)] — `proofs/R1-break.log` | `proofs/pass-cli.log` ok | code 6269de7b; R1–R10 test ba5a5f0c, lib tests 6269de7b |
| R2 `r2_policy_root_kinds_are_the_declaring_arms` | `classify_roots` lookup → `objects.rfind(id)` | `install-inserts in InterestingFirst` left [] — `proofs/R2-break.log` | `proofs/pass-cli.log` ok | code 6269de7b; R1–R10 test ba5a5f0c, lib tests 6269de7b |
| R3 `r3_d14_reads_the_declaring_arm` | `d14.rs classify_routine` lookup → last row | `one d14 in InterestingFirst: []` — `proofs/R3-break-classify.log` | `proofs/pass-cli.log` ok | code 6269de7b; R1–R10 test ba5a5f0c, lib tests 6269de7b |
| R3 (message site) | `d14.rs detect_d14` message lookup → last row | rootCause `DeadLive on Live Test is not called …` — `proofs/R3-break-message.log` | `proofs/pass-cli.log` ok | code 6269de7b; R1–R10 test ba5a5f0c, lib tests 6269de7b |
| R4 `r4_test_subtype_is_the_declaring_arms` | `test_coverage` Test-subtype check → `objects.rfind(id)` | `Helper in InterestingFirst` (0, 1) vs (1, 0) — `proofs/R4-break.log` | `proofs/pass-cli.log` ok | code 6269de7b; R1–R10 test ba5a5f0c, lib tests 6269de7b |
| R5 `r5_implicit_rec_binds_the_declaring_arms_source_table` | `workspace.rs resolve` → `objects.rfind(id)` | `OnOpenPage in InterestingFirst` ["Other Rec"] — `proofs/R5-break.log` | `proofs/pass-cli.log` ok | code 6269de7b; R1–R10 test ba5a5f0c, lib tests 6269de7b |
| R6 `r6_d60_reads_the_declaring_arms_subtype` | d60 membership → any row of the id | extra `("Rewrite", "Probe Batch")` — `proofs/R6-break.log` | `proofs/pass-cli.log` ok | code 6269de7b; R1–R10 test ba5a5f0c, lib tests 6269de7b |
| R7 `r7_d57_reads_the_declaring_arms_single_instance` | d57 `in_si` → any row of the id | extra `("PlainSub", "SI Plain")` — `proofs/R7-break.log` | `proofs/pass-cli.log` ok | code 6269de7b; R1–R10 test ba5a5f0c, lib tests 6269de7b |
| R8 `r8_d39_page_may_modify_reads_the_declaring_page` | `mod.rs page_may_modify` → last row | `d39- in InterestingFirst` left [] — `proofs/R8-break.log` | `proofs/pass-cli.log` ok | code 6269de7b; R1–R10 test ba5a5f0c, lib tests 6269de7b |
| R9 `r9_html_labels_name_the_declaring_arm` | `routine_label` → `m.objects` id map | `SI Keep :: KeepSub` missing (`SI Plain :: KeepSub`) — `proofs/R9-break-routine-label.log` | `proofs/pass-cli.log` ok | code 6269de7b; R1–R10 test ba5a5f0c, lib tests 6269de7b |
| R9 | `pub_label` → `m.objects` id map only | `Pub A` missing (`Pub B`) — `proofs/R9-break-pub-label.log` | `proofs/pass-cli.log` ok | code 6269de7b; R1–R10 test ba5a5f0c, lib tests 6269de7b |
| R9 | `pub_label` object fallback removed (B5) | `Probe Rec` missing (`…/Table/50190`) — `proofs/R9-break-fallback.log` | `proofs/pass-cli.log` ok | code 6269de7b; R1–R10 test ba5a5f0c, lib tests 6269de7b |
| R10 `r10_handler_functions_resolve_in_the_declaring_arm` | handler key → `(object_id, None)` (B4) | `Prod` covered — `proofs/R10-break.log` | `proofs/pass-cli.log` ok | code 6269de7b; R1–R10 test ba5a5f0c, lib tests 6269de7b |
| R11 `d50::tests::cap4_reads_the_declaring_arms_icb` | cap 4 lookup → last row | `routine at line 2` left true right false — `proofs/R11-break.log` | `proofs/pass-lib.log` ok | code 6269de7b; R1–R10 test ba5a5f0c, lib tests 6269de7b |
| R12 `return_summary::tests::bulk_icb_merge_reads_the_declaring_arm` | ICB lookup → `objects.rfind(id)` | left "normal" right "ignore" — `proofs/R12-break.log` | `proofs/pass-lib.log` ok | code 6269de7b; R1–R10 test ba5a5f0c, lib tests 6269de7b |
| R13a `root_classification::tests::split_arms_classify_against_their_own_row` | `classify_roots` lookup → `objects.rfind(id)` | `install_first=true` left [] — `proofs/R13a-break.log` | `proofs/pass-lib.log` ok | code 6269de7b; R1–R10 test ba5a5f0c, lib tests 6269de7b |
| R13b `root_classification::tests::per_id_classification_is_the_last_classified_rows` | break 1: per-id lookup only → `objects.rfind(id)` (old last-object) | both preconditions pass; final left ["upgrade-codeunit"] right ["install-codeunit"] — `proofs/R13b-break1-last-object.log` (R13a stays green under this break, as the spec predicted) | `proofs/pass-lib.log` ok | code 6269de7b; R1–R10 test ba5a5f0c, lib tests 6269de7b |
| R13b | break 2: per-id lookup only → `objects.find(id)` (first row) | final left ["upgrade-codeunit"] — `proofs/R13b-break2-first-row.log` | `proofs/pass-lib.log` ok | code 6269de7b; R1–R10 test ba5a5f0c, lib tests 6269de7b |
| R14 `cli_batch_census::*` | `PER_CONTEXT` back to `context.objects_by_id` | **stays green** (`proofs/R14-break.log`): not discriminating; the rename is a consistency edit, no pin (matrix "—") | `proofs/pass-cli.log` ok | code 6269de7b; R1–R10 test ba5a5f0c, lib tests 6269de7b |
| **Final panel r1 fixes (re-proofs on the changed tests/code; logs `proofs/r2-*.log`; pass = `r2b-task-affected.log`, 4373/4373)** | | | | |
| FS1 exit-code assert (R1's `policy_hits`) | test-side: append `--no-such-flag` to the policy args | fails at the new assert: `exit code of [...] in InterestingFirst: stderr=error: unexpected argument '--no-such-flag'` — `r2-FS1-exit-code.log` (shows the assert fires and names the exit; a JSON parse would also have failed here) | ok | 90ca96dd |
| R1 | `object_of` → last row | `foo-deletes in InterestingFirst` left [] — `r2-R1-break.log` | ok | 90ca96dd |
| R2 | `classify_roots` → `workspace.objects.rfind(id)` | `install-inserts in InterestingFirst` left [] — `r2-R2-break.log` | ok | 49a952d3, 90ca96dd |
| R3 | d14 test-name check → last row | `one d14 in InterestingFirst: []` — `r2-R3-break-classify.log` | ok | 90ca96dd |
| R3 | d14 message → last row | rootCause `DeadLive on Live Test …` — `r2-R3-break-message.log` | ok | 90ca96dd |
| R4 | Test-subtype check → last row | Helper (0, 1) — `r2-R4-break.log` | ok | 90ca96dd |
| R5 | `workspace.rs resolve` → last row | OnOpenPage ["Other Rec"] — `r2-R5-break.log` | ok | 90ca96dd |
| R6 | d60 → any row of the id | extra `Rewrite` — `r2-R6-break.log` | ok | 90ca96dd |
| R7 | d57 `in_si` → any row of the id | extra `PlainSub` — `r2-R7-break.log` | ok | 90ca96dd |
| R8 | `page_may_modify` → last row | `OnAction` lost — `r2-R8-break.log` | ok | 90ca96dd |
| R9 | `routine_label` → id map | `SI Keep :: KeepSub` missing — `r2-R9-break-routine-label.log` | ok | 90ca96dd |
| R9 | `pub_label` → id map only | `Pub A` missing (`Pub B`) — `r2-R9-break-pub-label.log` | ok | 90ca96dd |
| R9 | object fallback removed | `Probe Rec` missing (`…/Table/50190`) — `r2-R9-break-fallback.log` | ok | 90ca96dd |
| R10 (FC8 code) | `same_row` → always true (object id alone) | `Prod` covered — `r2-R10-break.log` | ok | 49a952d3, 90ca96dd |
| R11 (FO1 test) | cap 4 via `ctx.declaring` → last row | `routine at line 2` left true — `r2-R11-break.log` | ok | 49a952d3 |
| R13a (FC6 test) | `classify_roots` → `workspace.objects.rfind(id)` | `install_first=true` left [] — `r2-R13a-break.log` | ok | 49a952d3 |
| R13b (FC6 test) | break 1: per-id lookup → last row (anchor-cleared clone) | final left ["upgrade-codeunit"] — `r2-R13b-break1-last-object.log` | ok | 49a952d3 |
| R13b | break 2: per-id lookup → the row declaring the id's FIRST routine (first arm; no object slice there since FC6) | final left ["upgrade-codeunit"] — `r2-R13b-break2-first-row.log` | ok | 49a952d3 |
| **Final panel r2 fixes N1/N2/sol 2 (logs `proofs/r3-*.log`; pass = `r3-task-affected.log`, 4429/4429; rebased branch, base b229d970)** | | | | |
| R13a | `classify_roots` → `workspace.objects.rfind(id)` | `install_first=true` left [] — `r3-R13a-break.log` | ok | 729d046e |
| R13b | break 1: per-id core lookup → last row (anchor-cleared clone) | final left ["upgrade-codeunit"] — `r3-R13b-break1-last-object.log` | ok | 729d046e |
| R13b | break 2: per-id core lookup → row of the id's first routine | final left ["upgrade-codeunit"] — `r3-R13b-break2-first-row.log` | ok | 729d046e |
| R1 (N1 spawn helper) | `object_of` → last row | `foo-deletes in InterestingFirst` left [] — `r3-R1-break.log` | ok | 729d046e |

## Findings register summary

41 entries. Spec panel (19: S1–S10 sol, O1–O9 Opus): all `fixed`, accepted by both (converged at spec round 4 of 5). Final panel round 1 (19: FS1–FS3 sol, FO1–FO6 Opus, FC1–FC10 code-review skill at high): 16 `fixed`, FC1 `refuted` (Discovery 2), FC2 and FO6 `deferred` (minor; FC2 filed as Discovery 4). Final round 2 (3: FR2a–c): `fixed` in 729d046e. All accepted by both reviewers. Final rounds used: 3 of 5.

## Gate results

On H = `729d046e` (rebased on B = `b229d97051835ac3e8ad606961b6749cd5b0579e`), all supervised: ci-steps-all exit 0 (`logs/ci-steps-all/20261025T155111-42621c1a.log`); check-goldens-coverage exit 0 (`…/20261025T155841-ee62adf8.log`); check-goldens exit 0 (tree stamp: this exact tree passed `ci-steps test`; `…/20261025T155841-f47f0f7c.log`); cdo-gate first run exit 1 by OOM SIGKILL from a concurrent session (`…/20261025T155841-2117f4bd.log`), re-run alone exit 0 PASS (`…/20261025T160809-db2d33e4.log`).

master then moved twice (#111 `ce5ddc3d`, then #326 `0794b35a`, scripts/agentflow only); two clean rebases with `core.commentChar=';'`, each range-diff showing every branch commit `=` (unchanged); register SHAs remapped each time. Gates re-run on 633a1fcb (all four exit 0) and again on the final rebase.

**Final: H = `2b1cf934`, B = `0794b35aa12dbca0138b111f3f7f3d95d3718b32`**, all exit 0 and supervised: ci-steps-all (`logs/ci-steps-all/20261025T163317-4791eeca.log`), check-goldens-coverage (`…/20261025T163849-1bbcb3da.log`), check-goldens (`…/20261025T163849-0e361cfe.log`), cdo-gate PASS (`…/20261025T163850-11830fa2.log`, first run under #326's every-slot cdo-gate rule; no new oom_kill).

- `scripts/ci-steps task-affected` on the final tree (before the two commits; the commits hold exactly that tree): fmt, clippy-dev, gen-syntax ok; ".agent/ outside the map -> the whole suite": **4373 run, 4373 passed, 8 skipped**, exit 0 (`/work/bench/279/impl/task-affected.log`). No golden file changed (`git status` shows only the intended paths).
- `agentflow check-diff --base 3f5030f9 --head HEAD --issue 279`: `reasons: []` before commit 1, after commit 1, and on the final head ba5a5f0c.
- Commits: 6269de7b (Task 1: the swap, lib tests, census rename, plan doc), ba5a5f0c (Task 2: CLI tests R1–R10, CHANGELOG). No pre-commit hook ran (`core.hooksPath` is unset in this clone; no `--no-verify` used); the golden targets ran inside the task-affected whole-suite run on the identical tree.
- CDO re-measure on the final head ba5a5f0c (release-fast, `/work/bench/279/impl/after`), the pinned baseline @ bc3ccb18eb68, 0 dirty paths on both sides; base = `rev2/measure/before` (binary `/work/bench/279/before`, base 3f5030f9): `fingerprint.json`, `analyze-primary.json` (2,323 findings), `analyze-all.json` (30,618) **byte-identical**; opt-in d17/d46/d50 (`optin.sh`) at `--scope primary` and `--scope all` **byte-identical** to `/work/bench/279/optin-before` (6 findings each: d17 2, d46 1, d50 3 at all). Outputs in `/work/bench/279/impl/measure/after/`, `/work/bench/279/impl/optin-after/`.
- Final panel r1 fixes: commits 49a952d3 (FO1/FC5, FC6, FO3/FC3, FC7, FC8), 90ca96dd (FS1, FC9), 52bc5706 (CHANGELOG FS3/FO2/FC4; plan FS2/FC10), 1727e768 (FC6 follow-up: `PolicyModel::new` takes `&ModelEntities`, removing the `#[allow(clippy::too_many_arguments)]` that check-diff flagged as `forbidden:allow-attribute` on 49a952d3..52bc5706 — the three were committed before that output was read; nothing pushed). FO6 (shared test `span`) not done (optional). `task-affected` on 1727e768: 4373/4373, exit 0 (`r2b-task-affected.log`). `check-diff` on 1727e768: `reasons: []`. CDO on 1727e768 (release-fast, `/work/bench/279/impl/r2/`): fingerprint, analyze both scopes, opt-in d17/d46/d50 both scopes **byte-identical** to base; workspace bc3ccb18, 0 dirty.

## Discoveries

```json
[
 {"subsystem":"program graph / exports","locator":"src/program/build.rs dedup_objects; src/program/graphify_export.rs routine_label (:766); src/program/integration_report.rs object_name (:242)","symptom":"The program graph merges an #if-split object's arms into one ObjectNode that keeps the first arm's name, so graphify routine labels and integration-report object names give a routine of the second arm the first arm's name","kind":"bug","origin_issue":279,"reproducer":"#if A codeunit 50100 \"Foo\" { procedure P() ... } #else codeunit 50100 \"Bar\" { procedure Q() ... } #endif; the graphify export labels Q as Foo.Q()","pre_existing":true,"capability":"program-graph consumers name a routine's owner by its declaring arm","acceptance":"a split fixture shows Bar.Q() in the graphify export and integration report; the merged node's resolution facts are unchanged"},
 {"subsystem":"l5 d57","locator":"src/engine/l5/detectors/d57.rs cleared_by_object","symptom":"Clearing evidence is collected per object id, so a Clear in one SingleInstance #if arm suppresses d57 for a growing global in the other SingleInstance arm (false negative)","kind":"enhancement","origin_issue":279,"reproducer":"#if A codeunit 50100 \"SI A\" { SingleInstance = true; var L: List of [Text]; [EventSubscriber(...)] local procedure S() begin L.Add('x'); end; } #else codeunit 50100 \"SI B\" { SingleInstance = true; var L: List of [Text]; [EventSubscriber(...)] local procedure S2() begin L.Add('x'); end; procedure Drain() begin Clear(L); end; } #endif","pre_existing":true,"capability":"d57's clearing evidence is per declaring arm","acceptance":"the split fixture flags S in SI A (no clear in its own arm) and not S2"},
 {"subsystem":"program graph / name resolution","locator":"src/program/build.rs dedup_objects (the merged ObjectNode keeps the first arm's name); name-based object lookup in program/resolve","symptom":"A by-name reference to the SECOND arm's name of an #if-split object does not resolve: an event subscriber on Codeunit::\"Pub A\", where \"Pub A\" is the second arm, becomes the unresolved node unknown/codeunit/0:pub a in the event graph","kind":"bug","origin_issue":279,"reproducer":"#if A codeunit 50109 \"Pub B\" ... #else codeunit 50109 \"Pub A\" { [IntegrationEvent] procedure OnPub() } #endif plus [EventSubscriber(ObjectType::Codeunit, Codeunit::\"Pub A\", 'OnPub', ...)] elsewhere; `alsem analyze --format html` event graph shows node 'unknown/codeunit/0:pub a' on base 3f5030f9 and on the #279 prototype; with Pub A first it resolves","pre_existing":true,"capability":"a by-name reference to any #if arm's object name resolves to that object id","acceptance":"the reversed split fixture resolves the subscriber to the OnPub event; no unknown/codeunit node"},
 {"subsystem":"program model / declaring object","locator":"src/program/model/declaring.rs (DeclaringObjects::of, anchor containment with a last-row fallback); src/program/model/workspace.rs (assembly builds each routine inside its object's loop)","symptom":"After #279 every routine-scoped reader recomputes the routine's declaring object row by anchor containment (~15 readers, several DeclaringObjects builds per run), with a silent last-row fallback when no anchor contains the routine; a reader added later that uses an id map, or a row whose anchor is missing or overlapping, quietly goes back to last-wins","kind":"enhancement","origin_issue":279,"reproducer":"grep DeclaringObjects::new and .of( across src/engine and src/program/model after #279; compare with workspace.rs routine assembly, which knows each routine's object row when it pushes the routine","pre_existing":false,"capability":"each ModelRoutine carries its declaring object row (index) from assembly, so every reader is exact and O(1) and the anchor heuristic and its fallback retire","acceptance":"ModelRoutine stores its declaring row at assembly; DeclaringObjects::of becomes an O(1) read of it (or is removed); the #279 CLI and lib split-object tests stay green; CDO byte-identical"}
]
```

## Not delivered

- Proof scope (Opus final FO4): each single-site break reverts its site to the old lookup (last row, or any row for d57/d60), and each CLI test stops at its first failing order, so during implementation the second arm order of each CLI test was not itself seen failing; only the prototype's global B2 (first-row) run covers that direction, plus R13b break 2. The guards are not claimed as proven both ways per test.
- R9 matrix row (Opus final FO5): the implemented test asserts the subscriber labels in both arm orders, stronger than the row's "fixA only".
- d2, d13, d17, d46: swapped without a behaviour pin (d13/d17 no behaviour change by construction).
- Shared routine id (same signature in both arms): readers that look a routine up by id still see the last arm's answer (class P).
- R14 (census name) pins nothing: a consistency edit.
- d57 per-id clearing evidence (Discovery 2); program-graph owner names and by-name resolution of the second arm (Discoveries 1 and 3); split-table publisher label in HTML falls back to the last-wins row (stated in code and CHANGELOG); declaring row stored at assembly (Discovery 4, deferred FC2).
