# #160 child spec: step 5 — one coverage policy

Status: for spec panel round 1 (2026-10-22). Made from the pre-draft
`/work/bench/160-5/draft-step5-design.md` once the probes had run. §0 lists what the probes
changed.

Child of `2026-10-18-issue-160-analysis-session-design.md` (the epic): §1 goal 3, §5.5
amendment M5, §5.8 last line, §6 bullet "One coverage-fold policy" (called §6.3 below, as the
epic's row does), §9 row **5**, E3, E4, I2, I7, I9, I10, Decisions 25, 57, 58. The shape
follows `2026-10-22-issue-160-step11b-routine-rekeying-design.md`. Where this document and the
epic disagree, this document is wrong, EXCEPT the numbered amendments of §9 (5-A1 to 5-A8).
Each gives its reason; until the epic takes it, it is an open question for the panel.

Base: master `f96619ec` (step 11b's merge, #214), worktree `/work/al-sem-issue-160-a6`. Every
`file:line` below is in that tree. Every fact the design rests on is a row of the ledger's
`## Assumption probes` table (`.agent/issue-160/ledger.md`). The probe test, the scripts and
the raw output are in `/work/bench/160-5/` (`p1-lines.txt`, `run-base.sh`, `run-proto.sh`,
`run-d45.sh`, `comp.txt`, `p6-src/`, `proto.patch`). Nothing from the probes is on the branch.

Words:
- **Direct coverage** = a routine's own `(status, reasons)` from its body
  (`direct_facts_for_routine`, `capability_cone.rs:522`), before any fold. Its status is
  `complete` exactly when its reasons are empty (`:1112-1116`), `unknown` for a body-less or
  parse-incomplete routine (`:526-538`), else `partial`.
- **The four kinds** = the call-site-local uncertainty kinds `ambiguous-overload`,
  `member-not-found`, `external-target`, `interface-open-world`
  (`is_callsite_local_kind`, `db_effect_solver.rs:841-846`).
- **Fold** = adding the four kinds found on a routine's own uncertainty edges to its direct
  reasons, and turning `complete` into `partial` when a reason is new
  (`fold_uncertainty_reasons`, `capability_cone.rs:3654-3673`, over the scan
  `callsite_local_coverage_reasons`, `:3639-3648`). The fold never produces `unknown` (P3).
- **Inherited status** = the cone's roll-up over everything a routine reaches
  (`coverage_cone_for_scc`, `:2420-2471`; `inherited_status`, `:2945-2949`): `complete` only
  if every reached routine's direct status is `complete`.
- **Primary half / dependency half** = the uncertainty edges of workspace callers
  (`rel.primary().uncertainty_edges`) and of dependency callers (`rel.dependency_uncertainty()`,
  `exec_relation.rs:132-135`). Single-app, the dependency half is empty.
- **Producer** = code that builds a direct-coverage map and hands it to a cone. **Reader** =
  code that reads a cone's coverage result.
- **The prototype** = `/work/bench/160-5/proto.patch`, a throwaway change on the base that
  makes producers P-a and P-b (§3.1) fold both halves per row, exactly as §4.1 designs. It
  was built only to measure the moves before the panel (P8, P10-P14 "early"); it is not the
  implementation, and the acceptance runs repeat every number on the real head.

## 0. What the probes changed in the pre-draft

1. **No existing golden and no existing test moves** (P8 early, falsifies the draft's §6
   golden list). With the prototype, `scripts/check-goldens` passed 921 of 921 tests, and
   `cargo nextest run -p al-sem --lib` passed 2,241 of 2,241. So the bug the step fixes
   (#161 part 2) is witnessed by no test today, and the new tests of §5 are the only
   evidence. The epic row's golden list (r3a5, cli-b digest/prove, cli-c-events,
   cli-c-policy, r4, cli-a, gate, `cli_d35_unknown_commit`) becomes "none predicted"
   (amendment 5-A7). A golden that moves is now a discovery to explain (F5).
2. **The CDO move is small and clustered** (P10 early). Cross-app (the default), the
   prototype added 46 findings at `--scope primary` (d35 11 → 52, d9 34 → 39) and 88 at
   `--scope all` (d35 79 → 162, d9 34 → 39), removed none, and left every other default
   detector byte-identical. Two "partly analysed" hubs explain all 41 added d35 findings at
   primary scope: `ExecuteRequestInternal` (Codeunit 6225912, 35) and `LogMessageInternal`
   (Codeunit 8712, 6). The draft's "d35 UNKNOWN flood" is 41 findings and 2 root causes.
3. **d1 and d2 do not move on CDO** (falsifies the draft's prediction for 5-A1). They read
   coverage through `presence()` and CAN move, so they stay in the reader list with a pin
   (T9), but the triage has nothing to read for them.
4. **d45 is opt-in** (`detectors/mod.rs:1222`), so `cdo-measure`'s default `analyze` never
   runs it. Run explicitly (P10b): 150 findings both sides, 65 change only their text
   (`subscriberCoverage=complete` → `partial`). The measurement plan adds the explicit run.
5. **Single-app moves more, and by a different kind** (P11 early). `--single-app` analyze:
   d35 6 → 67, d9 27 → 37, two existing d35 findings gain a reason. Every one of the 61 new
   d35 findings names `external-target`: in single-app a call into a dependency has no body,
   so the call is `external-target` (741 Primary edges, 393 routines, P1). The snapshot
   already folds these in single-app; the context now agrees.
6. **The dependency half is 10.8 × the Primary half** (P1, flagged by its own rule). Cross-app
   four-kind edges: Primary 42 (19 routines ambiguous-overload, 15 interface-open-world),
   dependency 452 (106 / 19 external-target / 153 interface-open-world). `member-not-found`
   is 0 everywhere. It flips 965 primary routines alone, against 613 for the Primary half
   alone, 1,008 together (18.4 % of 5,479; P2). F3's 50 % trigger does not fire. F4 (§10)
   is the fallback shaped for this.
7. **The draft's T4 fixture shape resolves** (P6). The overload pair
   `I(A: Integer; B: Text)` / `I(A: Integer; B: Code[20])` called with a `Text` resolves
   (exact match); the `ws-overload-negatives` comment is stale. The `O(X: Interface ...)` /
   `O(X: Text)` pair called with a Codeunit is ambiguous; with it the dependency half holds
   exactly one `ambiguous-overload` edge, from `Inner`, and `Go` flips only under the
   dependency-half fold. T4 uses that shape.
8. **Policy** (P13). The bundled policy output is byte-identical with the prototype (as
   §3.4 predicts). Under `ws-policy-custom`'s fail-open `complete` gate, CDO cross-app loses
   182 of 255 `cust-coverage-gate` findings (`routinesSkippedCoverage` 1,228 → 2,236, the
   +1,008 equal to P2's flip count); single-app loses 113 of 159 (+1,588 skipped). That is
   the silent drop §3.4 describes, now measured.
9. **Identity holds** (P12, A5 early). With the prototype the CDO `aldump` histogram SHA
   (`c032c8db…`) and the single-app `fingerprint` bytes (`680d9ba8…`) are unchanged.
10. **11b changed nothing the design rests on** (P4 re-read). The solve keys
    `own_by_member` by `SolveIx` now (`db_effect_solver.rs:988`); a four-kind entry still
    goes only into the member's own list (`:1025`, `:1050`, `:1103`) and each member takes
    only its own (`:1143`). Every producer folds per row before the projection, and since a
    `complete` row has no reasons, folding before or after `least_complete`
    (`projection.rs:210-227`) gives the same per-id value.

## Feasibility first

Decision 58 classes step 5 **non-speed**: no bench, no wall or peak bound. Its CDO evidence is
`scripts/cdo-gate` (ratchets unchanged, I10) plus the identity check (SARIF at both scopes,
`aldump` histogram SHA). Unlike 11a/11b the step is EXPECTED to move findings; the move is
read by `cdo-measure` and `triage-findings`, per detector, as the row says.

On the measured data every acceptance item but one is feasible:
- **Feasible:** context == snapshot per root on r0 (T3); the dependency fixture checked
  against its own Cone (T4, P6 shape); the ambiguous-call and policy fixtures (T1, T2); the
  triage of every reader that moves (126 added or changed findings over 4 detectors and
  fanout in the default mode, two hubs at primary scope; §8); the histogram SHA.
- **Partial: "`policy check` with the owner's policy".** CDO has no `al-sem.policy.yaml`, and
  the bundled policy (`requireCoverage: partial`) cannot be moved by the fold (measured:
  byte-identical). The item is delivered with a stated proxy (Q1), not as worded.
- **The "> 30 % FP = opt-in or root-cause fix" rule** is feasible to apply; whether it fires
  is the triage's answer. Its fallbacks are pre-stated (F1-F4).

## 1. Goal

One fold, one input, every cone. Every producer of direct coverage applies the same fold to
the uncertainty of every caller its cone carries: the context and the cross-app base fold
`rel.primary().uncertainty_edges` AND `rel.dependency_uncertainty()`; the two snapshot
producers fold their own graph's edges (§2). Only the cone carries coverage up
(`Transfer::Cone`); no solve. After the step:
1. the detector context's per-routine coverage equals the snapshot's on every r0 fixture
   (single-app, where both read the same edges) — #161 part 2;
2. in cross-app the context and r3a5 also fold the dependency half, so a primary root that
   reaches dependency code with one of the four kinds is `partial`;
3. the test-only L4-solve fold (`compute_uncertainty_coverage_reasons`,
   `capability_cone.rs:3563-3616`) is deleted (epic §5.8 last line);
4. every reader that can move is listed (§3.2), pinned by a test of its USE, and its CDO move
   is triaged.

## 2. Non-goals

- **The snapshot's graph.** The snapshot keeps the cut graph (`compose_snapshot` →
  `build_r3a3_source_only_base`, `snapshot.rs:1239-1244`, `capability_cone.rs:4569-4576`)
  until step 6. Its fold input stays its own graph's uncertainty edges (Primary callers only,
  cross-app). See 5-A3 and Q2.
- **The L4 solve and the context's uncertainty indexes.** `uncertainty_edges_by_from`,
  `uncertainties_by_node`, the solve and the harvest stay Primary (epic M5). The fold is not
  the solve.
- **New kinds.** `unresolved-call`, `dynamic-dispatch`, `opaque-body`, `parse-incomplete` are
  not folded. Adding a kind is a change to `is_callsite_local_kind`, which moves the solver
  and the fold together (`db_effect_solver.rs:836-840`).
- **Resolver fixes.** A four-kind edge that the triage shows to be a resolver gap is filed as
  its own issue (F1 a); fixing it here would move the histogram SHA, which A6 forbids.
- **The program's dependency coverage** (`FreshCoverage`) and `return_summary.rs`'s
  `coverage`: same word, different concept; untouched.
- **Merging the two cross-app direct-coverage producers** (`det_cone_inputs` and
  `build_cross_app_base`). Both call the one fold; T5 pins that their maps are equal.
  Deleting one is step 6's or later's.
- **Roles** (step 8): the halves still split on `dep_routine_ids` by string id.
- **Speed.** No bench (Decision 58).

## 3. Background: who produces and who reads coverage today

### 3.1 Producers (direct coverage handed to a cone)

| # | Producer | Universe | Folds today? | Cone graph |
|---|---|---|---|---|
| P-a | `det_cone_inputs` (`detector_context.rs:1248-1285`), called by `build_detector_artifacts` (`:1362`) for every detector run, the policy RAW build (`gate/policy/pipeline.rs:266`), and the digest/events/query views (`session.rs:336`) | both | **no** | `rel.cone()` |
| P-b | `build_cross_app_base` (`capability_cone.rs:3856`), per-row loop `:3950-3979`, `direct_coverage` `:3980-3984` → `project_r3a5_cross_app` (`:4126`, cone `:4174-4181`) | cross-app | **no** | `base.rel.cone()` |
| P-c | `project_r3a3` (`:4353`, scan `:4391`, fold `:4407-4408`) | single-app | yes, own graph | own typed graph |
| P-d | `build_r3a3_source_only_base_over` (`:4583`, scan `:4607`, fold `:4620-4621`) → `compose_snapshot` → digest `coverage`, prove, fingerprint, diff, snapshot | both (cut) | yes, own graph | own typed graph |
| P-e | `compute_uncertainty_coverage_reasons` (`:3563-3616`, `#[cfg(test)]`): old side of step 3's oracle (`check_oracle`, `:6331`) and the roles-cap test (`:6630`) | test only | — (L4 solve) | — |

P-c/P-d fold, P-a/P-b do not: the #161 part-2 bug the epic names. Single-app, P-a's graph and
P-d's are built from the same calls, so their uncertainty edges are the same list.

### 3.2 Readers of the context's coverage (move with P-a)

| Reader | Reads | How it can move | CDO, prototype, default mode (P10) |
|---|---|---|---|
| transaction spans (`transaction_spans.rs:228-245`) | `reachable_coverage != "complete"` → `coverage_complete = false` | span becomes incomplete | via d9, digest |
| d9 (`detectors/d9.rs:56`, `:77`) | `span.coverage_complete` | new info finding | 34 → 39 (both scopes); single-app 27 → 37 |
| digest tx context (`digest_cli.rs:525`, `:537-538`) | `span.coverage_complete` | `TX_UNKNOWN` where it was known | not in `cdo-measure`; §8 step 4 |
| d35 (`detectors/d35.rs:64`, reasons `:233-268`) | `may_commit_derived` tri-state + `coverage.reasons` / `unknown_targets` | `No` → `Unknown`: new "may be reachable" findings; reason text grows | primary 11 → 52, all 79 → 162; single-app 6 → 67, 2 change text |
| d45, **opt-in** (`detectors/d45.rs:66`, `:110`) | `reachable_coverage` for publisher and subscribers | coverage text in the finding | 150 → 150, 65 change text (`--detector` run) |
| events fanout (`event_flow.rs:350`; `gate/events.rs:441`) | `inherited_status` → `capabilityComposition` | `complete` → `partial`; `--strict` exit codes | partial events 7 → 35; single-app 0 → 2 |
| policy coverage gate (`policy_engine.rs:250-264`, gate `:72-79`) | `inherited_status` | only under `requireCoverage: complete` (§3.4) | bundled: byte-identical; custom: §0 item 8 |
| d1 (`detectors/d1.rs:723`, `d1_graph.rs:135`) | `touches_db_derived` → `presence()` (`capability_query.rs:158-166`) | `No` → `Unknown`: the walk stops pruning there | 0 |
| d2 (`detectors/d2.rs:137`, `:392`) | `touches_physical_table_derived` → `presence()` | `No` → `Unknown`: more callees walked | 0 |
| d34 (`detectors/d34.rs:115`) | `may_commit_derived == Yes` | cannot move (the fold adds no fact) | 0 |

d8 and d50 read spans but not `coverage_complete`. prove reads the snapshot's coverage
(`prove.rs:211`), not the context's.

### 3.3 Readers of the snapshot's coverage (P-d; do not move)

prove, digest's `coverage` key, fingerprint's per-root `coverage` block, diff, the full
snapshot, `derive_coverage`. P-d already folds and its graph does not change, so their output
must stay byte-identical in both universes (A5; the prototype's CDO fingerprint is).

### 3.4 The policy gate

`passes_coverage_gate` (`policy_engine.rs:72-79`): `any` passes everything; `partial` fails
only `unknown`; `complete` fails `partial`. The fold only turns `complete` into `partial`, so a
gate at `partial` or `any` cannot move. The bundled policy sets `requireCoverage: partial` and
no rule overrides it (`policy-default.yaml:5`, P9). Under a `complete` gate the fold has two
outcomes:
- `onUnknown: fail-closed`: the rule's finding is replaced by a coverage finding
  (`emit_coverage_finding`), and the rule's facts are never evaluated (`continue`, `:263`).
- `onUnknown: fail-open`: the routine is counted in `routines_skipped_coverage` (`:254`) and
  emits nothing. A rule finding disappears with no trace except the counter. Measured on CDO
  with `ws-policy-custom`: −182 findings cross-app, −113 single-app (§0 item 8).

## 4. Design

### 4.1 One scan over any edges, one fold, per row (D1)

The two functions already exist; the change is their input and their callers.

```rust
// capability_cone.rs: the scan takes any edges (today it takes &PrimaryGraph).
pub(crate) fn callsite_local_coverage_reasons<'e>(
    edges: impl IntoIterator<Item = &'e UncertaintyEdge>,
) -> HashMap<&'e str, BTreeSet<&'e str>>
// fold_uncertainty_reasons: unchanged body, pub(crate).
```

Every producer calls the scan once, before its per-row loop, and folds each row with
`fold_uncertainty_reasons` before the projection (`merge_per_id` + `least_complete`), where
P-c and P-d already fold:

| Producer | Scan input |
|---|---|
| P-a `det_cone_inputs(resolved, rel)` | `rel.uncertainty_all()` |
| P-b `build_cross_app_base` (`rel` is built at `:3906`, before the loop at `:3950`) | `rel.uncertainty_all()` |
| P-c `project_r3a3` | `&graph.uncertainty_edges` (unchanged meaning) |
| P-d `build_r3a3_source_only_base_over` | `&graph.uncertainty_edges` (cut until step 6) |

```rust
// exec_relation.rs
/// Every caller's uncertainty edges: the Primary half, then the dependency half.
pub fn uncertainty_all(&self) -> impl Iterator<Item = &UncertaintyEdge> {
    self.primary.uncertainty_edges.iter().chain(&self.dependency_uncertainty)
}
```

**M5 decided: keep the two fields** (5-A4). The Primary indexes and the solve read only the
first half; one list with a role filter would make each of them filter, for the one reader
(the fold) that wants both. `dependency_uncertainty`'s "(no production reader yet)" doc lines
(`exec_relation.rs:38-41`, `:132`) are rewritten to name the fold.

Why per row, not a `&mut HashMap` pass after the projection (the draft's
`fold_direct_coverage`): it is where P-c/P-d fold today, so the four producers stay one shape,
and T3 compares like with like. The order does not change the result (§0 item 10). An edge
whose `from` has no row is ignored, as today; on CDO there are none in either half (P1).

`det_cone_inputs` gains `rel: &ExecRelation`. Callers: `build_detector_artifacts` (has `rel`),
and the tests `detector_context.rs:2449`, `detector_context_11b_tests.rs:169`, `:340`,
`capability_cone.rs:6944`, `:6962`, `:7259` (each builds or has a relation; the single-app ones
use `ExecRelation::single_app`).

### 4.2 Why folding the dependency half needs nothing else (D2)

A dependency routine already has a row in P-a's map (every model routine gets one,
`detector_context.rs:1267-1272`), and its own calls are typed edges in `rel.cone()` (step 4).
So folding its four-kind edges into its row is enough for every primary root that reaches it
through the Cone to inherit `partial`. No solve: the solve never moves a four-kind entry
between routines (P4), so a routine's own edges are its whole contribution.

### 4.3 Delete the L4-solve fold (D3, I2's exception)

Delete `compute_uncertainty_coverage_reasons` (`:3547-3616`, with its doc). Tests deleted in
the same commit, because they pin the deleted code (I2's exception):
- `check_oracle` (`:6331`) and its callers `callsite_local_scan_matches_the_fold_on_fixtures`
  (`:6477`) and `callsite_local_scan_matches_the_fold_on_cdo` (`:6568`, CDO-gated: the
  `cdo-gate` list loses one test). Their claim, "the scan equals the solve's fold", has no
  second side once the solve's fold is gone.
- `callsite_local_scan_matches_the_fold_under_a_roles_cap` (`:6630`): the same claim under a
  cap. No replacement (Q5): the new fold reads no solve and no cap.
- Kept unchanged except for the scan's new argument: `callsite_local_scan_hand_stated`
  (`:6422`), `snapshot_base_folds_the_scan_into_direct_coverage` (`:6597`),
  `snapshot_base_prints_no_capped_line` (`:6668`).

### 4.4 Determinism (I7)

The fold's reasons are a `BTreeSet`, sorted; the status is a function of the set; the scan
over two halves builds a set per routine, so the half order cannot change the output. No new
order.

### 4.5 Counters

No build is added or removed (I1 unaffected): the scan is a pass over edges the relation
already holds. The CDO counters stay those of 11b's final row.

## 5. Test plan

Every test lists its break (I9; the `discrimination-proof` skill: make the break with Edit,
watch the named assertion fail, revert with Edit, watch it pass, record both). Every break
removes or narrows the PRODUCTION fold call; none targets `fold_uncertainty_reasons` alone.
Because no existing test moves (§0 item 1), these are the step's only evidence.

| # | Test | Asserts | Break → failing assertion |
|---|---|---|---|
| T1 | **ambiguous-call fixture** (row's proof; inline, single-app, lib test over `build_detector_context`): entry root `Root` calls `Helper`; `Helper` makes the ambiguous `O(L)` call (§0 item 7) and writes a table; `Root` also commits (so a span exists) | `Helper` direct `partial`, reasons ⊇ `ambiguous-overload`; `Root` inherited `partial`; `Root`'s span `coverage_complete == false`; digest `transactionContext` for `Root` is `unknown`; d9 emits its incomplete-span finding for the span | delete the fold call in `det_cone_inputs` → `coverage_complete` is `true` |
| T2 | **policy fixture, both `onUnknown` outcomes hand-stated**: T1's workspace plus a policy with two rules matching `Helper`'s table write, both `requireCoverage: complete`, one `fail-closed`, one `fail-open` | rule A: exactly one coverage finding, on `Helper`, and no rule finding; rule B: no finding, `routinesSkippedCoverage` = the number of routines matched and `partial` (stated by name in the test), `routinesPassed` stated | delete the fold → A emits the rule finding instead; B's skipped count drops |
| T3 | **context == snapshot coverage per routine on r0** (row's acceptance): every `tests/r0-corpus/` dir, single-app, every routine: `(direct_status, inherited_status, sorted reasons, sorted unknown_targets)` from the context (`det_cone_inputs` + `compose_cone_over_graph` over `rel.cone()`) equals the snapshot base's (`build_r3a3_source_only_base`) | equality over all dirs; precondition: at least 11 dirs have a routine with a four-kind reason (P5) | delete the fold in `det_cone_inputs` → fails on those dirs (it fails the same way on the base: the test is the bug's witness) |
| T4 | **dependency fixture, the context against its own Cone** (row's acceptance): `write_fixture_dep_ambiguous` in `test_support.rs` (P6 shape: dependency `Dep Rec Lib::Outer` → `Inner`, `Inner` makes the ambiguous `O(L)` call and a `Modify`; workspace `Ws Call::Go` calls `Outer`) | cross-app context: `Inner` direct `partial`; `Go` inherited `partial` with `Inner` in `unknown_targets`; equals an independent BFS over `rel.cone()` from `Go` (partial iff a reached row has a non-complete direct status). Preconditions asserted: `rel.dependency_uncertainty()` holds exactly one `ambiguous-overload` edge, from `Inner`; `rel.primary().uncertainty_edges` holds none | fold `rel.primary().uncertainty_edges` only → `Go` is `complete` |
| T5 | **one fold, two cross-app producers**: on T4's workspace, P-a's `coverage_in` equals P-b's `base.direct_coverage` | map equality | drop the fold call in `build_cross_app_base` → inequality on `Inner` |
| T6 | **fanout** (E4): an event subscriber that reaches `Helper` | `events fanout --format json`: `capabilityComposition` `partial` for that event | delete the fold → `complete` |
| T7 | **d35** (E4): a subscriber reaching `Helper`, no commit | one d35 finding, UNKNOWN wording, reasons text contains `ambiguous-overload` | delete the fold → no finding |
| T8 | **d45** (E4; opt-in, so the test names `--detector`): publisher/subscriber pair where the subscriber reaches `Helper` and writes a table | the finding's root cause carries `subscriberCoverage=partial` | delete the fold → `complete` |
| T9 | **d1 and d2 presence** (5-A1): in T1's workspace, a caller whose only callee is `Helper`'s cone without a table fact in it (a read-free helper behind the ambiguous call) | `touches_physical_table_derived` and `touches_db_derived` on that callee's summary are `Unknown`, so d2's pre-filter (`d2.rs:137`) does not prune it | delete the fold → both `No` |

T6-T8 can be one workspace with three runs (the `cli_d35_unknown_commit` pattern), in the
`cli` umbrella. T1-T5 and T9 are lib tests. T4's fixture is shared with step 6's acceptance
(Q7).

`cli_d35_unknown_commit` (row's golden list): its variants use `object-run-unresolved`, a
direct-coverage reason, not a fold kind (P7), and the prototype left it green. Predicted
unchanged.

## 6. Acceptance matrix

| # | Claim | Evidence | Pass rule | Feasible on the data? |
|---|---|---|---|---|
| A1 | Context == snapshot per routine, r0, single-app | T3 | all dirs equal; break fails | yes |
| A2 | Cross-app fold includes the dependency half, carried by the Cone | T4, T5 | green; breaks fail | yes (P6 shape) |
| A3 | Every reader listed and pinned by its use | §3.2; T1 (spans, digest, d9), T2 (policy), T6 (fanout), T7 (d35), T8 (d45), T9 (d1, d2) | each break fails | yes |
| A4 | No existing golden or test outcome moves except the I2-exception deletions of §4.3 | `scripts/check-goldens`, `scripts/ci-steps task` on the head | green with no regen; a move is F5 | yes (prototype: 921/921, 2,241/2,241) |
| A5 | Snapshot-side output byte-identical | the A4 goldens (r3a3, cli-b fingerprint/snapshot/diff/prove); CDO `fingerprint` bytes, parent vs head | 0 bytes moved | yes (prototype: equal) |
| A6 | Resolution untouched | CDO `aldump --program-call-graph-stats` SHA, parent vs head | equal (`c032c8db…` on the parent) | yes |
| A7 | CDO ratchets | `bash scripts/cdo-gate` | exit 0 | yes |
| A8 | CDO moves triaged | `cdo-measure` comp + explicit d45 + `--single-app` runs; `triage-findings` on every added or changed finding (§8) | each detector ≤ 30 % FP on its added findings in the default mode at both scopes, or the pre-stated fallback (F1-F4) applied | yes (126 default-mode items; primary scope 2 root causes) |
| A9 | Policy | CDO `policy check`, bundled policy, both universes: byte-identical to the parent. Proxy for "the owner's policy" (Q1): `ws-policy-custom`, both universes: deltas of findings and `routinesSkippedCoverage` reported; a seeded sample of 30 dropped `cust-coverage-gate` routines checked to reach a four-kind edge | as stated | **partial**: CDO has no owner policy; the bundled one cannot move (§3.4) |
| A10 | L4-solve fold gone | `grep -rn compute_uncertainty_coverage_reasons src` | 0 hits | yes |

## 7. False-positive and failure shapes

- **d35 "may be reachable" findings.** The largest reader move: +41 primary, +83 all
  cross-app, +61 single-app (prototype). Primary scope is two hubs; all scope is 12. The
  triage criterion is pre-stated so it cannot drift: an added d35 UNKNOWN finding is
  **true** if, in the real source (dependency source included), a Commit is reachable from the
  subscriber, OR a named uncertain call cannot be decided statically (an interface whose
  implementations are not all in the closure, a callee with no body in the closure); it is
  **false** if every target of every named uncertain call is in the closure and none reaches
  a Commit. F1.
- **Single-app `external-target`.** Every new single-app d35 finding names `external-target`
  (calls into Continia dependency apps the mode excludes). Honest for the mode; likely false
  by the criterion above, since the dependency usually does not commit. `--single-app` is
  itself the opt-in mode (the default is cross-app since S8), so these are reported and
  sampled, not gated (Q8).
- **d9 info findings** for spans newly incomplete (+5 cross-app, +10 single-app). Info
  severity; triaged with the same criterion on the span's named reasons.
- **d45 text** (opt-in): `subscriberCoverage` complete → partial on 65 findings; no count
  change.
- **d1/d2 walk more** where a callee becomes `Unknown`: findings can be added, none removed
  by the fold. 0 on CDO; T9 pins the arm.
- **Policy fail-open silent drop** (§3.4): measured −182 cross-app under the custom policy.
  Reported, never hidden. It is the declared meaning of `fail-open`, so it is not counted as
  FP; the sample in A9 checks that each dropped routine really reaches a four-kind edge.
- **Cross-app digest/prove disagreement until step 6** (5-A3): the snapshot's `coverage` can
  say `complete` while the span says `unknown`, for a root whose only four-kind edge is in
  dependency code (P2: 965 primary routines flip from the dependency half alone). Stated;
  closed by step 6's Cone snapshot.
- **A collided id** (11b): every row of the id folds the same reasons, and the projection
  takes the least complete, so the per-id value never becomes more complete.
- **An edge `from` with no row**: ignored, as today; 0 on CDO (P1).

## 8. Measurement plan

Non-speed (Decision 58): no bench. Binaries: release-fast `alsem` + `aldump` at the parent
(`f96619ec`, already in `/work/bench/160-5/parent/`) and at the head, copied out of `target/`.
The CDO path is read at run time (`jq -r .cdo_ws /work/alsem/.agent/config.json`). Each run in
the background, output to a log file; never piped through `tail`.

1. `cdo-measure run` both sides, then `comp`. Expected (prototype): fingerprint unchanged;
   analyze changes only in d35 and d9, counts as §3.2.
2. d45 explicitly (opt-in): `alsem analyze <WS> --detector d45-event-transitive-table-exposure`
   at both scopes and `--single-app` (`/work/bench/160-5/run-d45.sh`). Expected: 150 → 150,
   text moves only.
3. `--single-app` analyze, both sides (`run-proto.sh`'s loop). Expected: d35 6 → 67, d9 27 → 37.
4. `alsem digest` on the CDO roots whose span flips (from step 1's d9 adds): `transactionContext`
   `unknown` where it was known; reported.
5. `alsem events fanout <WS> --format json --deterministic`, both universes: per-event
   `capabilityComposition` diff; every flipped event must reach a four-kind edge (P14).
6. `alsem policy check`, bundled and `ws-policy-custom`, both universes (A9).
7. `aldump --program-call-graph-stats` SHA both sides (A6).
8. Triage (A8): `triage-findings` on every added d35 and d9 finding of the default mode, both
   scopes (≤ 88 + 5), and on the d45 text changes, root cause first (one hub, one verdict, then
   spot-check each finding of the hub); single-app: a seeded sample of 30 (seed recorded).
   FP rate per detector, per universe, per scope.
9. `bash scripts/cdo-gate` once, as the branch gate.

## 9. Amendments this spec asks the epic to take

- **5-A1** §6.3's reader list adds d1, d1_graph, d2 (through `presence()`), d9 (through
  spans) and digest's tx context; names d45 as opt-in; names d34 as unable to move (reads
  `Yes` only).
- **5-A2** The row's "`policy check` with the owner's policy" is delivered with the proxy of
  Q1. Under the bundled policy (`requireCoverage: partial`) the fold cannot move the gate.
- **5-A3** Until step 6 the snapshot's fold input is its own cut graph's uncertainty (Primary
  callers, cross-app). The context and r3a5 fold both halves. Cross-app digest and prove may
  show `coverage: complete` beside `transactionContext: unknown` for a root whose only
  four-kind edge is in dependency code. Step 6's acceptance closes it.
- **5-A4** M5 is decided: two fields kept, one chained accessor `uncertainty_all()`.
- **5-A5** §5.8's deletion of `compute_uncertainty_coverage_reasons` happens here, with the
  tests named in §4.3.
- **5-A6** E4's fixture list adds d9 (T1), d1/d2 presence (T9) and the policy fail-open counter
  (T2).
- **5-A7** The row's golden list ("r3a5, cli-b digest/prove, cli-c-events, cli-c-policy, r4,
  cli-a, gate, `cli_d35_unknown_commit`") becomes "none predicted" (§0 item 1): the existing
  goldens do not witness the bug, and the new tests carry the evidence. The epic's "Goldens
  move in both universes" (§6.3) is withdrawn.
- **5-A8** The triage gate of the row (> 30 % FP) applies to the default (cross-app) mode at
  both scopes; `--single-app` moves are reported and sampled (Q8).

## 10. Fallbacks (each pre-stated; firing one needs no new review round)

- **F1 (d35 > 30 % FP on its added findings, default mode, either scope).**
  (a) Split the false ones by the hub and the uncertain call that made them `unknown`. Each
  cluster whose call is a resolver gap (the target is statically decidable from the closure)
  is filed as its own issue (a resolver change moves the histogram, A6), and recorded in the
  ledger's discoveries. (b) Then, whatever (a) found, d35's UNKNOWN arm (every reason, the
  existing `object-run-unresolved` findings included) moves to a new opt-in detector id
  `d35u-commit-may-be-reachable-in-event-subscriber`; d35 keeps its YES arm, default, and its
  YES findings stay byte-identical. The opt-in id is added to `OPT_IN_DETECTORS` and to the
  ledger's discoveries as "back to default when (a)'s issues land". Rejected: letting d35
  treat a fold-only `Unknown` as `No` (it re-creates the two policies the step removes).
- **F2 (d1, d2 or d9 > 30 % FP on added findings, default mode).** The same root-cause split
  as F1 (a). If no cluster explains it, the detector ships opt-in for the step (CLAUDE.md
  rule) and a filed issue fixes the root cause. Same rejection as F1.
- **F3 (any detector has > 300 added findings in one mode and scope, or the head's P2 count
  exceeds 50 % of primary routines cross-app).** Triage a seeded random sample of 30 per
  detector (seed recorded) instead of every finding; the FP rule applies to the sample.
  (The prototype predicts neither: max 83, 18.4 %.)
- **F4 (the dependency half alone causes a detector's > 30 % FP and the Primary half does
  not).** Decide by re-running the triage's false findings against a head built with the
  dependency half dropped from P-a/P-b's scan input. If F4 holds: land the fold with the
  Primary half in both universes now, keep `uncertainty_all()` unused, and move the dependency
  half to step 6 with the snapshot's Cone graph, as epic amendment 5-A9 (A2/T4/T5 move to step
  6). This is the only fallback that changes the step's scope; it is pre-stated here so that
  the panel's acceptance of this spec is the owner's acceptance of the move.
- **F5 (an existing golden or test outcome moves, other than §4.3's deletions).** Stop.
  Explain the move from the diff (the `golden-diff-triager` agent) before regenerating
  anything; a move traced to the fold in a reader §3.2 lists is regenerated and recorded as a
  discovery against §0 item 1; any other move is a bug to fix. No regen is a bless.

## 11. Risks

- **The move is honest but user-visible.** Some answers that said `complete` now say
  `partial`. The old `complete` was wrong; the triage is the gate.
- **Snapshot/context mismatch in cross-app until step 6** (5-A3).
- **The hubs.** Two routines explain all primary-scope d35 adds; if both are false, d35 is
  41/41 false at primary scope and F1 fires. That is a likely outcome, so F1 is written to
  fire without a new round.

## 12. Open questions for the spec panel (recommended answers)

1. **Q1. Which is "the owner's policy"?** CDO has no `al-sem.policy.yaml`. *Recommended:*
   bundled policy must be byte-identical (measured on the prototype), plus
   `tests/fixtures/cli-c-policy/ws-policy-custom/al-sem.policy.yaml` as the `complete`-gate
   proxy (the 7a bench's choice), reported with the A9 sample, not FP-gated.
2. **Q2. Should the snapshot fold the dependency half now?** *Recommended: no, wait for step
   6* (5-A3). The cut graph has no dependency-internal edges, so folding now would reach only
   dependency routines called directly by primary code, and it would move the snapshot
   goldens and fingerprint that A5 keeps fixed.
3. **Q3. Opt-in for one arm of d35.** *Recommended:* the split of F1 (b), a new opt-in id
   for the UNKNOWN arm, rather than all of d35 opt-in (that would hide the YES arm, which the
   fold cannot change) or a config flag (no detector has one).
4. **Q4. Is folding `external-target` from dependency code right?** Cross-app it is 19 edges
   and 238 flips; only one added d35 finding names it alone. *Recommended: yes*; the callee
   has no body in the universe. `unfetched-dependency` (step 6) is a ledger row, not a
   coverage signal.
5. **Q5. Replace the deleted roles-cap test?** *Recommended: no.* The new fold reads no solve
   and no cap; `callsite_local_scan_hand_stated` pins the scan.
6. **Q6. Own use-pinning tests for d1 and d9?** *Recommended: yes, cheaply*: d9 is asserted in
   T1 (it moves on CDO) and d1 with d2 in T9 (no CDO move, but the arm can move). No existing
   golden would catch either (§0 item 1).
7. **Q7. T4's fixture is the one step 6 reuses?** *Recommended: yes*:
   `write_fixture_dep_ambiguous` in `test_support.rs` beside `write_fixture_dep_uncertainty`
   (whose `R.NoSuchMethod()` yields `unresolved-call`, not a fold kind, so it cannot serve).
8. **Q8. Which mode does the > 30 % rule gate?** *Recommended:* the default (cross-app) mode
   at both scopes (5-A8). `--single-app` is opt-in already, and its 61 new d35 findings all
   name `external-target` into apps the mode excludes; sample 30, report the rate, do not
   gate.
