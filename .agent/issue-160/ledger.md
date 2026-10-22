# Issue #160 ledger: attempt 6 on server1 (epic step 5: one coverage policy)

## Claim

```json
{"run_id":"20261022-213444-4c7bee","issue":160,"attempt":6,"session":"claude-code-session:c8325c25-f1bf-436d-a204-90152a41120c","branch":"issue/160-alsem-batch-build-the-model-onc-a6","worktree":"/work/al-sem-issue-160-a6","body_hash":"2ec746e00deded290965d462f57e8ade73a96d606db1b7f89361f532a6d32883","title":"alsem batch: build the model once and run several commands against it","reconciled":[]}
```

Issue body hash: `2ec746e00deded290965d462f57e8ade73a96d606db1b7f89361f532a6d32883`.
Base `f96619ecf27d777b6f3769661e4145ff0c5ee5d5` (make-worktree, origin/master; step 11b's merge, #214).
Operator: goal "Finish #160 as fast as possible with as few interactions as possible; merge is allowed" (2026-10-22);
epic step 5 next. Reviewers: the operator's standing stand-ins gpt-6.1-sol (slot astra) and Opus (slot flash); attest
with `--substitute astra=sol --substitute flash=opus`. Pre-draft: `/work/bench/160-5/draft-step5-design.md`, probes
`/work/bench/160-5/probes-todo.md` (static half pre-run on master 455ac2d1). The previous step's ledger and register
(11b) stay in git history at `f96619ec`.

## Classification

`architectural`: step 5 changes which uncertainty every coverage producer folds, in both universes (one fold
for four producers, the dependency half newly read), deletes the test-only L4-solve fold, and moves user-visible
findings (d1, d2, d9, d35, d45, fanout, policy under a `complete` gate) that need CDO triage.

## Assumption probes

Base `f96619ec`; base binaries (release-fast) in `/work/bench/160-5/parent/`. P1/P2 ran as a CDO-gated
`#[ignore]` lib test in the throwaway base worktree only (`src/engine/l4/step5_probe.rs`, never on the branch);
raw lines in `/work/bench/160-5/p1-lines.txt`. The "proto" rows ran a throwaway fold prototype
(`/work/bench/160-5/proto.patch`: P-a and P-b fold `rel.primary().uncertainty_edges` + `rel.dependency_uncertainty()`
per row) built to `/work/bench/160-5/proto/`, to size the detector moves before code; they are predictions, not
the acceptance runs.

| probe | result | evidence |
|---|---|---|
| P1 dependency half vs Primary half (flag if dep > 10x Primary) | **FLAGGED**: cross-app four-kind edges, Primary half 42 (ambiguous-overload 23 from 19 routines, interface-open-world 19 from 15), dependency half 452 (ambiguous-overload 134/106 routines, external-target 19/19, interface-open-world 299/153) = 10.8x. `member-not-found` is 0 everywhere. Single-app Primary: ambiguous-overload 21, external-target 741 (393 routines), interface-open-world 19. Every four-kind edge's `from` has a coverage row (0 without, both halves) | `p1-lines.txt` lines `P1 ...`; probe source kept at `/work/bench/160-5/step5_probe.rs` (base worktree removed) |
| P2 roots `complete` -> `partial` (F3 if > 50 % of primary cross-app) | holds: cross-app 1,008 of 5,479 primary routines flip (18.4 %; 23.7 % of the 4,251 complete); Primary half alone 613, dependency half alone 965; by kind (both halves): interface-open-world 949, ambiguous-overload 374, external-target 238, member-not-found 0. Single-app 1,588 (29.0 %), external-target alone 1,553. Subscribers flipped: 46 of 89 complete cross-app, 63 of 91 single-app | `p1-lines.txt` lines `P2 ...` |
| P3 fold never writes `unknown` | holds on `f96619ec`: fold `capability_cone.rs:3654-3673` only `complete`->`partial`; roll-up `:2420-2471` (`:2435-2442`) and `inherited_status` `:2945-2949` give only `complete`/`partial`; the `"unknown"` at `:4289-4290` is the r3a3 no-cone fallback, not a fold path | grep `"unknown"` in `capability_cone.rs` |
| P4 SCC-member filter after 11b | holds: `own_by_member: HashMap<SolveIx, _>` (`db_effect_solver.rs:988`), four-kind entries go only to `own` (`:1025`, `:1050`, `:1103`), inserted per member (`:1118`), each member takes only its own (`:1143`); no copy between members | read `db_effect_solver.rs:980-1150` on `f96619ec` |
| P5 11 r0 dirs carry a four-kind reason | holds: 11 | loop over `tests/r3a3-goldens/*.json` |
| P6 a dependency routine can emit `ambiguous-overload` | holds, with the shape changed: the draft's `I(A: Integer; B: Text)` overload pair RESOLVES (exact Text match; `aldump --dependency-bodies-stats` `ambiguousResolved` 0); the `O(X: Interface ...)` / `O(X: Text)` pair called with a Codeunit is ambiguous. With it, `rel.dependency_uncertainty()` = exactly 1 `ambiguous-overload` from `Inner`; `Go` flips only under the dependency-half fold; single-app `Go` flips by `external-target` | `/work/bench/160-5/p6-src/Lib.al`, `p6-dep.json`, `p6-probe.log` |
| P7 `cli_d35_unknown_commit` names no fold kind | holds (`:7,:196,:212,:217` `object-run-unresolved` only) | grep |
| P9 bundled policy has no `complete` gate | holds (`policy-default.yaml:5` `requireCoverage: partial`, nothing else) | grep |
| P8 early: which golden families move (draft predicted r3a5, cli-b digest, cli-c-events, cli-c-policy, r4, cli-a, gate) | **FALSIFIED**: with the prototype, `scripts/check-goldens` 921/921 pass and `cargo nextest run -p al-sem --lib` 2,241/2,241 pass; no golden and no test witnesses the bug today. Spec §0.1, §5, 5-A7, A4, F5 | `/work/bench/160-5/proto-goldens.json` + its slot log; `proto-lib.json` |
| P10 early: analyze moves only in d1, d2, d9, d35, d45 | **partly falsified**: cross-app d35 11->52 (primary) / 79->162 (all), d9 34->39 both; 0 removed; every other default detector byte-identical, d1 and d2 included. d45 is OPT-IN (`detectors/mod.rs:1222`) so `cdo-measure` never runs it; explicit run: 150->150, 65 text-only changes (`subscriberCoverage=partial`). Added d35 at primary scope: 2 hubs (`ExecuteRequestInternal` Codeunit 6225912 x35, `LogMessageInternal` Codeunit 8712 x6). Spec §0.2-0.4, §3.2, §8 step 2 | `measure/{before,after}/`, `comp.txt`, `d45-*.json` |
| P11 early: single-app moves too | holds: d35 6->67 (every new one names `external-target`), d9 27->37, 2 existing d35 gain a reason; other detectors byte-identical. Spec §0.5, Q8 | `sa-{parent,proto}-primary.json` |
| P12 early: resolution untouched | holds: histogram SHA `c032c8db...` parent == prototype; single-app `fingerprint` bytes `680d9ba8...` equal | `hist-p.json`, `hist-proto.json`, `p2-fp*.json` |
| P13 policy under a `complete` gate | measured: bundled policy byte-identical. `ws-policy-custom` cross-app `cust-coverage-gate` findings 255->73, `routinesSkippedCoverage` 1,228->2,236 (+1,008 = P2); single-app 159->46, 1,224->2,812 (+1,588 = P2). Fail-open silent drop is real. Spec §0.8, A9 | `pol-*.json` |
| P14 early: fanout moves | cross-app `coveragePartialEvents` 7->35, single-app 0->2; per-event four-kind check left for the head (spec §8 step 5) | `fan-*.json` |
| P2 (snapshot side, as written) | fingerprint objects with `inheritedStatus` and a four-kind reason: 2,637 (parent; the snapshot already folds) | `p2-fp.json` jq |

## Design

## Base/Head SHAs

- B (base, `origin/master`): `f96619ecf27d777b6f3769661e4145ff0c5ee5d5` (step 11b's merge, #214)
- H (gated head): `e7818178` (b04b8551's code + the FQ-02 numbers-file wording). The first gate run on b04b8551
  recorded no head for `ci-steps-all` and `cdo-gate` because e7818178 was committed mid-run, so both were re-run on H.
- After H: the evidence commit (ledger + register) only

## Timeline

- Claimed (attempt 6), worktree from `f96619ec`.
- Probes run (P1, P2, P6 + base/prototype sides of P8, P10-P14); spec committed `071d1c81`
  (`docs/superpowers/specs/2026-10-22-issue-160-step5-coverage-policy-design.md`); `check-diff` no reasons.
- Spec panel round 1 (sol 7I/4m, opus 6I/10m) -> 19 merged findings R1-01..R1-19, all `fixed`; spec fix `5acb6cfb`;
  `check-diff` no reasons. Register `.agent/issue-160/findings.json`.
- Spec panel round 2 (opus accepted 19; sol accepted 16, re-raised R1-03/R1-05/R1-06, new R2-01; opus new R2-02,
  R2-03) -> all fixed in `ad291462` (d2 budget: T9 restored + F6; F1 mixed-reason text permitted; F2 needs an
  incomplete span; span reasons defined; non-detector moves reported, not gated); `check-diff` no reasons.
- Spec panel round 3 (opus accepted 6; sol re-raised R1-05, new R3-01, both on F6) -> fixed in `62d2593d`: T9-main /
  T9-nested with their own mutations; F6 outcome-based; residual rule = stated limit + d2 budget-free-reachability
  discovery issue, fold kept. `check-diff` no reasons. Round 4: converged (23 entries, all accepted).
- Task 1 committed `63801ccd` (one fold for P-a/P-b, `uncertainty_all`, L4-solve fold + 3 tests and their helper `check_oracle` deleted, CHANGELOG):
  `ci-steps task-affected` exit 0 (3672 run, 3672 passed, 8 skipped); `check-goldens` exit 0, no regen (926 passed,
  4 skipped); `ci-steps clippy` exit 0; `check-diff` no reasons. Proof rows below.
- Task 1 review (sonnet): ACCEPT, 4 minors fixed forward in `99b8d4b3` (CHANGELOG wording x2, drop comment, code-map
  note); `task-affected` exit 0 (3672 passed, 8 skipped).
- Task 2: head binaries built from a clean detached worktree at `63801ccd`; comparison runs, span probe, edge probe,
  digest/prove all through the bench slot, every run exit 0. Every CDO move equals the prototype's. Triage: d35 UNKNOWN
  0/41 (primary), 2/83 (all) epistemically false; d9 0/5; no fallback fires. Numbers file committed `9d0662a7`
  (`docs/measurements/2026-10-22-issue-160-step5.md`); `check-diff` no reasons. Licensing `cdo-gate` left to the
  coordinator on the final head.
- Final panel round 1 (sol, opus, code-review; all confirm the fold): FQ-01..FQ-15 fixed in `b04b8551` (A9 sample,
  hazard wording, T9/T1/T4/T6/cap-test evidence, docs, FQ-14 stated limit), CR-01/02/05 refuted by design, CR-06
  deferred (D5). Register +19 entries (unreviewed). `task-affected`, `check-goldens`, release clippy exit 0;
  `check-diff` no reasons.

### Acceptance tests (step 6)

Written on base `f96619ec` (+ spec commits), uncommitted (they go in with the implementation). Lib tests in
`src/engine/l5/detector_context_step5_tests.rs` (module `tests_step5`); CLI tests in
`tests/cli/cli_step5_coverage_fold.rs`; T4/T5 fixture `write_fixture_dep_ambiguous` in `src/engine/l5/test_support.rs`.
Runs: `cargo test -p al-sem --lib tests_step5`, `cargo test --test cli cli_step5_coverage_fold::`. Each test asserts
its hand-stated preconditions first (they hold on the base); every red below is the missing fold. Sanity check: with a
throwaway fold (the prototype's shape, applied by Edit and reverted by Edit; `git diff` clean afterwards) all 11
pass, T5 calling the throwaway's `det_cone_inputs(resolved, Some(rel))`. F6 has not fired, so T9-main/T9-nested and
the F1/F2 migration tests are not written.

| test | file | fails because | output excerpt (base) |
|---|---|---|---|
| T1 `step5_t1_spans_and_d9_read_the_fold` | lib | `det_cone_inputs` does not fold: `Helper` stays `complete` (so the span is complete, d9 silent) | `Helper's direct coverage folds its ambiguous call: ... left: ("complete", []) right: ("partial", ["ambiguous-overload"])` |
| T1b `step5_t1b_digest_and_prove_transaction_context` | cli | the tx map reads a complete span | `digest: the COMMIT effect's transactionContext left: ["span-has-known-writes"] right: ["unknown"]` (digest `coverage` key already `partial`: asserted before, passes) |
| T2 `step5_t2_policy_coverage_gate_both_on_unknown_modes` | cli | `Helper` passes the `complete` gate | `A (fail-closed): (matched, skippedCoverage) left: (Some(1), Some(0)) right: (Some(0), Some(1))` |
| T3 `step5_t3_context_coverage_equals_the_snapshot_on_r0` | lib | context unfolded, snapshot folded (#161 part 2) | `53 routine(s) differ over 217 dirs ... ws-bare-implicit-rec ...: snapshot Some(("partial", "partial", ["ambiguous-overload"], ...)), context Some(("complete", "complete", [], []))`; precondition ">= 11 dirs with a four-kind reason" holds |
| T4 `step5_t4_dependency_half_folds_into_the_cross_app_context` | lib | the dependency half is not folded | `Inner's direct coverage folds its dependency-half edge: ... left: ("complete", []) right: ("partial", ["ambiguous-overload"])`; preconditions (one dep-half edge from Inner, none Primary, no object run) hold |
| T5 `step5_t5_both_cross_app_producers_fold_the_same` | lib | neither cross-app producer folds (the maps are equal on the base, so the test first asserts Inner's folded row in both) | `(P-a, P-b) Inner's row is folded in both producers left: (Some(("complete", [])), Some(("complete", []))) right: (Some(("partial", ["ambiguous-overload"])), Some(("partial", ["ambiguous-overload"])))` |
| T6 `step5_t6_events_fanout_coverage_policy_modes` | cli | the subscriber's inherited status is `complete` | `warn: OnS5's capabilityComposition ... left: Some(String("complete")) right: Some(String("partial"))` |
| T7 `step5_t7_d35_unknown_commit_from_the_fold` | cli | `mayCommit` is NO | `one d35 finding: [] left: 0 right: 1` |
| T8 `step5_t8_d45_subscriber_coverage_from_the_fold` | cli | subscriber coverage `complete` | `the subscriber coverage is partial: Publisher ... subscriberCoverage=complete` |
| T9 main `step5_t9_d2_budget_cost_main` | lib | `Wide` is `No`, pruned, so d2 reaches `Db` and emits | `the walk spends its budget under the partial Wide ... (Wide's inherited status: complete) left: ["d2/r0/0aa6.../loop0/.../event/one"] right: []` |
| T9 control `step5_t9_d2_budget_cost_control` | lib | **passes now** (must stay green on base and head) | `ok` |

Implementer notes: T5 calls `det_cone_inputs(&base.resolved)`; when §4.1 adds `rel`, it becomes
`det_cone_inputs(&base.resolved, &base.rel)` (one more caller for §4.1's list). T3 reads the context through
`build_detector_context` (which runs `det_cone_inputs` + the cone over `rel.cone()`), not by calling
`det_cone_inputs` directly, so it needs no change.

## Acceptance matrix

| # | Result | Evidence |
|---|---|---|
| A1 | delivered | T3 (217 r0 dirs equal; break A fails it) |
| A2 | delivered | T4, T5 (P6 O-shape fixture); breaks B, C fail them |
| A3 | delivered | T1 (spans, d9), T1b (digest, prove), T2 (policy), T6 (fanout), T7 (d35), T8 (d45), T9 (d2 budget); reader-side breaks for T1/T4/T6 (FQ-04); d1 stated limit (§3.2) |
| A4 | delivered | `check-goldens` 926 passed, no golden moved; only the three I2-exception tests (+ helper) deleted |
| A5 | delivered | single-app goldens unchanged; CDO fingerprint bytes equal; cross-app digest `coverage` key equal on all flipped roots |
| A6 | delivered | CDO histogram SHA `c032c8db…` equal, parent vs head |
| A7 | delivered | `cdo-gate` on H (gate-runs record) |
| A8 | delivered | numbers file: every move as predicted; d35 epistemic FP 0/41 primary, 2/83 all; d9 0/5; fanout/spans/d45 reported; no fallback fired |
| A9 | delivered (proxy, Q1) | bundled policy byte-identical both universes; `ws-policy-custom` drops 182/113, seeded sample 30 per universe, 0/30 false (FQ-01) |
| A10 | delivered | `grep -rn compute_uncertainty_coverage_reasons src`: 0 hits |

## Discrimination proofs

| Test | Mutation | Fail | Pass | Commit |
|---|---|---|---|---|
| T1 `step5_t1_spans_and_d9_read_the_fold` | A: `det_cone_inputs` pushes `(status, reasons)` unfolded (fold call deleted) | `:193` "Helper's direct coverage folds its ambiguous call" left `("complete", [])` right `("partial", ["ambiguous-overload"])` (first assertion after the preconditions; the span/d9 asserts follow it) | ok after revert | `63801ccd` |
| T1b `step5_t1b_digest_and_prove_transaction_context` | A | `cli_step5_coverage_fold.rs:179` "digest: the COMMIT effect's transactionContext" left `["span-has-known-writes"]` right `["unknown"]` | ok | `63801ccd` |
| T2 `step5_t2_policy_coverage_gate_both_on_unknown_modes` | A | `:261` "A (fail-closed): (matched, skippedCoverage)" left `(Some(1), Some(0))` right `(Some(0), Some(1))` | ok | `63801ccd` |
| T3 `step5_t3_context_coverage_equals_the_snapshot_on_r0` | A | `:313` "53 routine(s) differ over 217 dirs" (e.g. `ws-bare-implicit-rec`: snapshot `partial`/`ambiguous-overload`, context `complete`) | ok | `63801ccd` |
| T6 `step5_t6_events_fanout_coverage_policy_modes` | A | `:329` "warn: OnS5's capabilityComposition" left `"complete"` right `"partial"` | ok | `63801ccd` |
| T7 `step5_t7_d35_unknown_commit_from_the_fold` | A | `:396` "one d35 finding: []" left 0 right 1 | ok | `63801ccd` |
| T8 `step5_t8_d45_subscriber_coverage_from_the_fold` | A | `:430` "the subscriber coverage is partial: ... subscriberCoverage=complete" | ok | `63801ccd` |
| T9 main `step5_t9_d2_budget_cost_main` | A | `:617` "the walk spends its budget under the partial Wide ... (Wide's inherited status: complete)" left `["d2/r0/0aa6.../loop0/.../event/one"]` right `[]` | ok | `63801ccd` |
| T9 control `step5_t9_d2_budget_cost_control` (control, not a discrimination) | A | still ok (finding emitted), as the spec requires | ok | `63801ccd` |
| T4 `step5_t4_dependency_half_folds_into_the_cross_app_context` | B: `det_cone_inputs` scans `&rel.primary().uncertainty_edges` only | `:392` "Inner's direct coverage folds its dependency-half edge" left `("complete", [])` right `("partial", ["ambiguous-overload"])` (asserted before `Go`'s inherited status; same cause). T5 also fails on P-a; T1/T3/T9 stay green (single-app) | ok | `63801ccd` |
| T5 `step5_t5_both_cross_app_producers_fold_the_same` | C: `build_cross_app_base` pushes `(status, reasons)` unfolded | `:463` "(P-a, P-b) Inner's row is folded in both producers" left `(Some(("partial", ["ambiguous-overload"])), Some(("complete", [])))`; every other step-5 test green | ok | `63801ccd` |
| changed caller: `tests::collision_rules_hand_stated` (detector_context.rs), `tests_11b::collision_rules_hand_stated`, `tests_11b::set_valued_surfaces_do_not_depend_on_row_order` (now pass `ExecRelation::single_app`) | D: `det_cone_inputs`'s facts projection takes the first row (`v.into_iter().next().unwrap_or_default()`) instead of `union_by_value` | `detector_context.rs:2459` "R2: the direct facts are the union of the rows'"; `11b_tests.rs:175` (B's modify in the direct facts); `:467` "surface det_cone_inputs.direct_full differs between row orders" | ok | `63801ccd` |
| changed caller: `callsite_local_scan_hand_stated` (scan now takes `&graph.uncertainty_edges`) | E1: the scan's `is_callsite_local_kind(kind) \|\| true` | `capability_cone.rs:6318` map `assert_eq` fails (dynamic-dispatch / unresolved-call keys appear) | ok | `63801ccd` |
| kept: `snapshot_base_folds_the_scan_into_direct_coverage` (P-d's call site changed) | E2: P-d (`build_r3a3_source_only_base_over`) skips the fold | `:6354` "Plain: direct status complete ... no reason ambiguous-overload in []" for Plain/Loop/Ping | ok | `63801ccd` |
| T9 main, extended (FQ-03: each Ai's ambiguous edge as precondition, folded coverage after the d2 assert) | A: `det_cone_inputs` pushes rows unfolded | preconditions hold (each Ai's one uncertain call is ambiguous-overload); fails at `:645` "the walk spends its budget under the partial Wide ... (Wide's inherited status: complete)"; control ok | ok | `b04b8551` |
| T1, span reader side (FQ-04) | `transaction_spans.rs`: `reachable_coverage(..) != "complete" && false` (span ignores coverage) | `:203` "the span reaches Helper's ambiguous call, so it is incomplete" | ok | `b04b8551` |
| T1, d9 reader side (FQ-04) | `d9.rs`: `!span.coverage_complete && false` in `effects_are_interesting` | `:213` "d9 emits exactly one finding, for Root's span: []" left 0 right 1 | ok | `b04b8551` |
| T4, Cone reader side (FQ-04) | `coverage_cone_for_scc`: successor `!yc.complete && false` (no roll-up of partial) | `:400` "Go inherits partial" left `"complete"` right `"partial"` (Inner's direct assertion passes first) | ok | `b04b8551` |
| T6, strict reader side (FQ-04) | `gate/events.rs` strict: `capability_composition == "partial" && false` | `cli_step5_coverage_fold.rs:345` "strict: the partial entry is dropped" left `Some({capabilityComposition: partial, ..})` right `None` (warn mode passes first) | ok | `b04b8551` |
| `snapshot_base_prints_no_capped_line` (FQ-05: cap-fires precondition) | `build_r3a3_source_only_base_over` runs `compute_summaries_v2` and `eprintln!`s `render_warnings` | `:6437` printed == [] fails: "warning: summarize: Summary fixed-point did not converge for SCC [...]"; the new preconditions hold | ok | `b04b8551` |

## Findings register summary

42 entries: spec panel R1-01..R3-01 (23, 4 rounds) and final panel FQ-01..FQ-15, CR-01, CR-02, CR-05, CR-06 (19, 3
final rounds; sol, Opus, code-review at high). 0 open; refuted with evidence: CR-01, CR-02, CR-05 (by spec design);
deferred: CR-06 (minor, discovery D5). Every entry accepted by both reviewers. Stated limit added: FQ-14 (D4).

## Gate results

| Head | Gate | Exit | Result |
|---|---|---|---|
| `63801ccd` | `ci-steps task-affected` | 0 | 3672 passed, 8 skipped |
| `63801ccd` | `check-goldens` (no regen) | 0 | 926 passed, 4 skipped |
| `63801ccd` | `ci-steps clippy` (release) | 0 | no warnings |
| `99b8d4b3` | `ci-steps task-affected` | 0 | 3672 passed, 8 skipped |
| `9d0662a7` | `check-diff` | 0 | `reasons: []` |
| `9d0662a7` | `ci-steps all`, `cdo-gate`, `check-goldens` (coordinator, agentflow) | 0 | see `g5-*.json` |
| `b04b8551` | `ci-steps task-affected` | 0 | 3672 passed, 8 skipped |
| `b04b8551` | `check-goldens` (no regen) | 0 | 926 passed, 4 skipped |
| `b04b8551` | `ci-steps clippy` (release) | 0 | no warnings |
| `b04b8551` | `check-diff` | 0 | `reasons: []` |

## Discoveries

```json
[
  {"subsystem": "resolver (interface dispatch)", "locator": "src/engine/l4/combined_graph.rs interface-open-world emission", "symptom": "An interface declared Access = Internal is emitted as interface-open-world although it is implementable only in its own app (and friends): 23 such interfaces give 109 cross-app and 9 single-app edges on CDO, and alone cause 1 epistemically false d35 UNKNOWN finding (#160 step 5 discovery D1).", "kind": "enhancement", "origin_issue": 160, "pre_existing": true, "reproducer": "CDO cross-app: CSC Subscription Cache Subscr.OnAfterSubscriptionStatusChanged reaches only CSC ISubscription Cache (Internal); docs/measurements/2026-10-22-issue-160-step5.md D1.", "capability": "an Internal interface dispatch closed over the closure's implementations (conditionally resolved), not open-world", "acceptance": "no interface-open-world edge for an Internal interface whose implementations are all in the closure; the d35 item above drops"},
  {"subsystem": "resolver (overload resolution)", "locator": "src/program/resolve (this-qualified call overload choice)", "symptom": "A this-qualified call the AL compiler decides is resolved as ambiguous-overload (157 cross-app / 21 single-app ambiguous-overload edges on CDO; alone causes 1 epistemically false d35 UNKNOWN finding; #160 step 5 discovery D2).", "kind": "bug", "origin_issue": 160, "pre_existing": true, "reproducer": "this.IsSalesDocument(eDocCreationFactory.GetBusinessDocumentHeader().Number) in Codeunit 6225538 (Continia Delivery Network, eDocCreationUtil.Codeunit.al:26).", "capability": "pick the overload the compiler picks for this call shape", "acceptance": "the call resolves to one target; the d35 item CTS-CDN API Test Mode Subsc.OnBeforeSendNetworkDocument drops"},
  {"subsystem": "cross-app resolution (dependency bodies)", "locator": "src/program/resolve (Resolution::ExternalTarget for a same-app callee)", "symptom": "A call between two routines of the same dependency app, both with symbols and shipped source, resolves through the ABI only (external-target) cross-app (#160 step 5 discovery D3).", "kind": "bug", "origin_issue": 160, "pre_existing": true, "reproducer": "CDO cross-app: CSC Onprem Licn. Mgt.GetDatabaseReader (Codeunit 6192854) -> CSC DB Reader.TryGetDatabaseReader (Codeunit 6192855), Continia Core.", "capability": "resolve such a call to the callee's loaded body", "acceptance": "the edge is resolvedSource, not external-target; no step-5 verdict changes"},
  {"subsystem": "#160 step 6 (dependency L4 solve)", "locator": "src/engine/l4 cross-app solve over rel.primary(); r3a5 projection", "symptom": "A dependency routine's solved summary never carries its own four-kind calls (uncertainties [], hasUnresolvedCalls false) while its folded coverage (step 5) is partial; only the r3a5 projection prints both (#160 step 5 FQ-14, discovery D4; predates step 5; stated limit).", "kind": "bug", "origin_issue": 160, "pre_existing": true, "reproducer": "aldump --r3a5-cross-app-summary on the write_fixture_dep_ambiguous shape: Inner directStatus partial, reasons [ambiguous-overload], uncertainties [], hasUnresolvedCalls false.", "capability": "the dependency half solved with its own uncertainty edges, or the r3a5 projection stating the split", "acceptance": "Inner's summary carries the ambiguous-overload uncertainty on that fixture"},
  {"subsystem": "#160 step 6 (one cross-app producer)", "locator": "src/engine/l5/detector_context.rs det_cone_inputs; src/engine/l4/capability_cone.rs build_cross_app_base", "symptom": "Cross-app, det_cone_inputs recomputes the per-row direct facts and folded coverage build_cross_app_base already stored (direct_full, direct_coverage); T5 guards drift (#160 step 5 discovery D5, code-review item deferred).", "kind": "enhancement", "origin_issue": 160, "pre_existing": false, "reproducer": "any cross-app analyze: both producers run over the same rel.", "capability": "one producer whose maps the detector context reads", "acceptance": "the context reads the base's maps cross-app; T5 becomes structural"}
]
```

## Not delivered

Nothing in the acceptance matrix. A9's "owner's policy" is delivered through the agreed proxy (Q1), since CDO has no
policy file. Deferred by design: the snapshot's dependency-half fold (5-A3, step 6), the dependency L4 solve (D4, step
6), one cross-app coverage producer (D5, step 6).
