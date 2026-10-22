# #160 child spec: step 5 — one coverage policy

Status: revised after spec panel round 1 (2026-10-22; findings R1-01 to R1-19 in
`.agent/issue-160/findings.json`). Made from the pre-draft
`/work/bench/160-5/draft-step5-design.md` once the probes had run. §0 lists what the probes
changed; §0.1 lists what round 1 changed.

Child of `2026-10-18-issue-160-analysis-session-design.md` (the epic): §1 goal 3, §5.5
amendment M5, §5.8 last line, §6 bullet "One coverage-fold policy" (called §6.3 below, as the
epic's row does), §9 row **5**, E3, E4, I2, I7, I9, I10, Decisions 25, 57, 58. The shape
follows `2026-10-22-issue-160-step11b-routine-rekeying-design.md`. Where this document and the
epic disagree, this document is wrong, EXCEPT the numbered amendments of §9 (5-A1 to 5-A8).
Each gives its reason; until the epic takes it, it is an open question for the panel.

Base: master `f96619ec` (step 11b's merge, #214), worktree `/work/al-sem-issue-160-a6`. Every
`file:line` below is in that tree. Every fact the design rests on is a row of the ledger's
`## Assumption probes` table (`.agent/issue-160/ledger.md`). The probe test, the scripts and
the raw output are in `/work/bench/160-5/` (`p1-lines.txt`, `step5_probe.rs`, `run-base.sh`,
`run-proto.sh`, `run-d45.sh`, `comp.txt`, `p6-src/`, `proto.patch`). Nothing from the probes is
on the branch.

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
  if every reached routine's direct status is `complete` and no reached routine is an
  unresolved object-run source (`:2432-2435`).
- **Primary half / dependency half** = the uncertainty edges of workspace callers
  (`rel.primary().uncertainty_edges`) and of dependency callers (`rel.dependency_uncertainty()`,
  `exec_relation.rs:132-135`). Single-app, the dependency half is empty.
- **Producer** = code that builds a direct-coverage map and hands it to a cone. **Reader** =
  code that reads a cone's coverage result, directly or through the transaction spans.
- **The prototype** = `/work/bench/160-5/proto.patch`, a throwaway change on the base that
  makes producers P-a and P-b (§3.1) fold both halves per row, exactly as §4.1 designs. It
  was built only to measure the moves before the panel (P8, P10-P14 "early"); it is not the
  implementation, and the acceptance runs repeat every number on the real head.
- **Epistemic verdict / hazard verdict** (§7): whether a `partial` answer is CORRECT for the
  universe analysed, and whether the real code (dependency source included) actually has the
  hazard the finding is about.

## 0. What the probes changed in the pre-draft

1. **No existing golden and no existing test moves** (P8 early, falsifies the draft's golden
   list). With the prototype, `scripts/check-goldens` passed 921 of 921 tests, and
   `cargo nextest run -p al-sem --lib` passed 2,241 of 2,241. So the bug the step fixes
   (#161 part 2) is witnessed by no test today, and the new tests of §5 are the only
   evidence. The epic row's golden list becomes "none predicted" (amendment 5-A7). A golden
   that moves is a discovery to explain (F5).
2. **The CDO move is small and clustered** (P10 early). Cross-app (the default), the
   prototype added 46 findings at `--scope primary` (d35 11 → 52, d9 34 → 39) and 88 at
   `--scope all` (d35 79 → 162, d9 34 → 39), removed none, and left every other default
   detector's findings AND `detectorStats` byte-identical (d1 and d2 included). Two "partly
   analysed" hubs explain all 41 added d35 findings at primary scope:
   `ExecuteRequestInternal` (Codeunit 6225912, 35) and `LogMessageInternal` (Codeunit 8712, 6);
   12 hubs explain the 83 at all scope. Every one of the 41 names `interface-open-world`.
3. **d1 and d2 do not move on CDO, and cannot move their output** (§3.2, the d1/d2 limit).
   They stay in the reader list as readers whose cost, not output, changes.
4. **d45 is opt-in** (`detectors/mod.rs:1636-1648`), so `cdo-measure`'s default `analyze`
   never runs it. Run explicitly (P10b): 150 findings both sides, 65 change only their text
   (`subscriberCoverage=complete` → `partial`). §8 adds the explicit run.
5. **Single-app moves more, and by a different kind** (P11 early). `--single-app` analyze:
   d35 6 → 67, d9 27 → 37, two existing d35 findings gain a reason. Every one of the 61 new
   d35 findings names `external-target`: in single-app a call into a dependency has no body,
   so the call is `external-target`. The snapshot already folds these in single-app; the
   context now agrees.
6. **The dependency half is 10.8 × the Primary half** (P1, flagged by its own rule). Cross-app
   four-kind EDGES: Primary half 42 (ambiguous-overload 23, interface-open-world 19),
   dependency half 452 (ambiguous-overload 134, external-target 19, interface-open-world 299).
   Distinct `from` ROUTINES: Primary 19 / 0 / 15, dependency 106 / 19 / 153. `member-not-found`
   is 0 everywhere. Single-app Primary: 741 `external-target` edges from 393 routines. The
   dependency half alone flips 965 primary routines, the Primary half alone 613, both 1,008
   (18.4 % of 5,479; P2). F3's 50 % trigger does not fire.
7. **The draft's T4 fixture shape resolves** (P6). The pair `I(A: Integer; B: Text)` /
   `I(A: Integer; B: Code[20])` called with a `Text` resolves (exact match); the
   `ws-overload-negatives` comment is stale. The pair `O(X: Interface ...)` / `O(X: Text)`
   called with a Codeunit is ambiguous to our resolver (the AL compiler would reject it: see
   T4's leniency note); with it the dependency half holds exactly one `ambiguous-overload`
   edge, from `Inner`, and `Go` flips only under the dependency-half fold.
8. **Policy** (P13). The bundled policy output is byte-identical with the prototype (§3.4).
   Under `ws-policy-custom`'s fail-open `complete` gate, CDO cross-app loses 182 of 255
   `cust-coverage-gate` findings (`routinesSkippedCoverage` 1,228 → 2,236, the +1,008 equal to
   P2's flip count); single-app loses 113 of 159 (+1,588 skipped).
9. **Identity holds where it can be measured** (P12). With the prototype the CDO `aldump`
   histogram SHA (`c032c8db…`) and the `fingerprint` bytes (`680d9ba8…`; fingerprint is
   single-app only, `fingerprint_cli.rs:372-389`) are unchanged.
10. **11b changed nothing the design rests on** (P4 re-read). The solve keys
    `own_by_member` by `SolveIx` now (`db_effect_solver.rs:988`); a four-kind entry still goes
    only into the member's own list (`:1025`, `:1050`, `:1103`) and each member takes only its
    own (`:1143`). Every producer folds per row before the projection, and since a `complete`
    row has no reasons, folding before or after `least_complete` (`projection.rs:210-227`)
    gives the same per-id value.

### 0.1 What spec panel round 1 changed

- **prove moves** (R1-01): prove reads the context's transaction map and prints
  `transactionContext`; it joins §3.2, gets a pin (T1b) and a measurement (§8 step 4).
- **A5 states only what can be measured** (R1-02): fingerprint, prove and diff are single-app
  only; cross-app snapshot identity is read off `digest`'s `coverage` key.
- **T1, T2 rebuilt** (R1-03, R1-04): T1 gets `Entry → Root` so d9's span has two routines, and
  hand-stated preconditions; T2 scopes both rules with `routine.name` and states literal
  counters through the production `policy check` pipeline.
- **T9 replaced by a stated limit** (R1-05): d1/d2's `presence()` arms decide only whether a
  walk enters a callee whose cone holds no fact of the asked kind, so they cannot change
  output; §3.2 states the argument and the CDO evidence.
- **F1 narrowed** (R1-06): only UNKNOWN findings whose every reason is a fold kind move to the
  opt-in id; #148's `object-run-unresolved` findings stay default; migrations pre-stated.
- **Triage verdicts pre-stated per kind and per reader** (R1-07, R1-08, R1-09): §7.
- **F4 no longer drops the dependency fold** (R1-10).
- **T6 pins `--coverage-policy`** (R1-11); counts, citations, deleted-doc lines, §7 pointer,
  Q2's reason, T4's assertions and the fixture note fixed (R1-12 to R1-19).

## Feasibility first

Decision 58 classes step 5 **non-speed**: no bench, no wall or peak bound. Its CDO evidence is
`scripts/cdo-gate` (ratchets unchanged, I10) plus the identity check, read for this step as:
SARIF at both scopes identical for every detector except the moving ones (d35, d9 in the
default set; d45's text when run explicitly), and the `aldump` histogram SHA identical. Unlike
11a/11b the step is EXPECTED to move findings; the move is read by `cdo-measure` and
`triage-findings`, per detector, as the row says.

On the measured data every acceptance item but one is feasible:
- **Feasible:** context == snapshot per routine on r0 (T3); the dependency fixture checked
  against its own Cone (T4, P6 shape); the ambiguous-call and policy fixtures (T1, T1b, T2);
  the triage of every reader that moves. Default mode, prototype: 88 added findings at all
  scope (83 d35 + 5 d9; 46 at primary scope, 41 + 5), 28 fanout events flipped, and the
  changed span coverage records (§8 step 4); plus 65 opt-in d45 text changes. The primary
  scope's d35 adds are 2 root causes.
- **Partial: "`policy check` with the owner's policy".** CDO has no `al-sem.policy.yaml`, and
  the bundled policy (`requireCoverage: partial`) cannot be moved by the fold (measured:
  byte-identical). Delivered with a stated proxy (Q1), not as worded.
- **The "> 30 % FP = opt-in or root-cause fix" rule** is feasible to apply with §7's verdicts;
  whether it fires is the triage's answer. Its fallbacks are pre-stated (F1-F3).

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
4. every reader that can move its output is listed (§3.2), pinned by a test of its USE, and
   its CDO move is triaged.

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
- **Resolver fixes.** A four-kind edge that the triage shows to be a resolver gap (§7) is
  filed as its own issue; fixing it here would move the histogram SHA, which A6 forbids. The
  same holds for emitting `interface-open-world` less often (it is emitted for every resolved
  interface dispatch site, `combined_graph.rs:369-389`, whatever the interface's access).
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
| P-a | `det_cone_inputs` (`detector_context.rs:1248-1285`), called by `build_detector_artifacts` (`:1362`) for every detector run, the policy RAW build (`gate/policy/pipeline.rs:266`), and the digest/prove/events/query views (`session.rs:336`) | both | **no** | `rel.cone()` |
| P-b | `build_cross_app_base` (`capability_cone.rs:3856`), per-row loop `:3950-3979`, `direct_coverage` `:3980-3984` → `project_r3a5_cross_app` (`:4126`, cone `:4174-4181`) | cross-app | **no** | `base.rel.cone()` |
| P-c | `project_r3a3` (`:4353`, scan `:4391`, fold `:4407-4408`) | single-app | yes, own graph | own typed graph |
| P-d | `build_r3a3_source_only_base_over` (`:4583`, scan `:4607`, fold `:4620-4621`) → `compose_snapshot` → the snapshot-derived keys of digest, prove, fingerprint, diff, snapshot | both (cut) | yes, own graph | own typed graph |
| P-e | `compute_uncertainty_coverage_reasons` (`:3563-3616`, `#[cfg(test)]`): old side of step 3's oracle (`check_oracle`, `:6331`) and the roles-cap test (`:6630`) | test only | — (L4 solve) | — |

P-c/P-d fold, P-a/P-b do not: the #161 part-2 bug the epic names. Single-app, P-a's graph and
P-d's are built from the same calls, so their uncertainty edges are the same list.

### 3.2 Readers of the context's coverage (move with P-a)

| Reader | Reads | How it can move | CDO, prototype, default mode (P10) |
|---|---|---|---|
| transaction spans (`transaction_spans.rs:228-245`) | `reachable_coverage != "complete"` over the span's routines → `coverage_complete = false` | span becomes incomplete | enumerated in §8 step 4 |
| d9 (`detectors/d9.rs:46-59`, `:77`) | `span.coverage_complete`, on spans with ≥ 2 routines (`:23`, `:46`) | new info finding when the span has < 2 pending physical tables | 34 → 39 (both scopes); single-app 27 → 37 |
| digest tx context (`digest_cli.rs:525-545`; the map from `finish_default_run`, `:53-58`) | `span.coverage_complete` | `transactionContext` `unknown` where it was known, on every explicit-Commit span | §8 step 4 |
| **prove** tx context (`prove.rs:886`, `:960`; printed through `digest_cli::effect_to_value`, `prove.rs:665`, `digest_cli.rs:1104`) | the same tx map | `transactionContext` of a COMMIT effect in `evidence` | §8 step 4 |
| d35 (`detectors/d35.rs:64`, reasons `:233-268`) | `may_commit_derived` tri-state + `coverage.reasons` / `unknown_targets` | `No` → `Unknown`: new "may be reachable" findings; reason text grows | primary 11 → 52, all 79 → 162; single-app 6 → 67, 2 change text |
| d45, **opt-in** (`detectors/d45.rs:66`, `:110`) | `reachable_coverage` for publisher and subscribers | coverage text in the finding | 150 → 150, 65 change text (`--detector` run) |
| events fanout (`event_flow.rs:350`; `gate/events.rs:441`; `--coverage-policy`, `:456-518`) | `inherited_status` → `capabilityComposition` | `complete` → `partial`; under `--coverage-policy strict` the entry is dropped with a stderr line and exit 1; under `ignore` it is rewritten to `complete` | partial events 7 → 35; single-app 0 → 2 |
| policy coverage gate (`policy_engine.rs:250-264`, gate `:72-79`) | `inherited_status` | only under `requireCoverage: complete` (§3.4) | bundled: byte-identical; custom: §0 item 8 |
| d1 (`d1_graph.rs:135`, the closure edge filter; `d1.rs:723` is the test-only shadow oracle) | `touches_db_derived` → `presence()` (`capability_query.rs:158-166`) | walk cost only (the d1/d2 limit below) | 0 findings, 0 `detectorStats` change |
| d2 (`detectors/d2.rs:137`, `:392`) | `touches_physical_table_derived` → `presence()` | walk cost only | 0 findings, 0 `detectorStats` change |
| d34 (`detectors/d34.rs:115`) | `may_commit_derived == Yes` | cannot move (the fold adds no fact) | 0 |

**The d1/d2 limit (stated, not pinned).** `presence()` returns `Unknown` instead of `No` only
for a summary whose cone holds no fact of the asked kind. Each arm that reads it (d1's closure
edge filter, d2's `expand` at `:137`, d2's subscriber gate at `:392`) only decides whether a
walk ENTERS that callee. The walk follows the Primary graph's edges (a subset of the Cone's)
and its terminals are record operations in the routines it reaches; every such operation is
already a fact in the callee's cone. So a walk that enters a callee with no fact of that kind
ends with no terminal, and d1/d2 output cannot change; only the walk's cost can. There is
therefore no output a use-pinning test could assert. Evidence: the prototype left d1 and d2
findings and `detectorStats` byte-identical on CDO at both scopes, and every r4 golden green.
If the head moves either, it is F5.

d8 and d50 read spans but not `coverage_complete`.

### 3.3 Readers of the snapshot's coverage (P-d; do not move)

prove's snapshot-derived keys (`coverage_complete` from `entry.coverage_status`, `prove.rs:211`;
not its `transactionContext`, §3.2), digest's `coverage` key, fingerprint's per-root
`coverage` block, diff, the full snapshot, `derive_coverage`. P-d already folds and its graph
does not change, so these keys must stay byte-identical. fingerprint, prove and diff are
single-app only (`--single-app` exists on query, policy check, events, analyze and digest
only); the cross-app snapshot is read by digest's `coverage` key (A5).

### 3.4 The policy gate

`passes_coverage_gate` (`policy_engine.rs:72-79`): `any` passes everything; `partial` fails
only `unknown`; `complete` fails `partial`. The fold only turns `complete` into `partial`, so a
gate at `partial` or `any` cannot move. The bundled policy sets `requireCoverage: partial` and
no rule overrides it (`policy-default.yaml:5`, P9). The gate runs per workspace routine BEFORE
facts (`:250-264`), after the applicability check, which can skip a routine only on a
routine-scope predicate (a fact-scope field is `Unknown` in applicability mode,
`predicate_evaluator.rs:219-221`; only `False` skips, `policy_engine.rs:239`). A routine that
fails the gate always counts in `routines_skipped_coverage` (`:254`), in both modes; then:
- `onUnknown: fail-closed`: a coverage finding replaces the rule's finding
  (`emit_coverage_finding`), and the rule's facts are never evaluated (`continue`, `:263`).
- `onUnknown: fail-open`: nothing is emitted. A rule finding disappears with no trace except
  the counter. Measured on CDO with `ws-policy-custom`: −182 findings cross-app, −113
  single-app (§0 item 8).

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
/// The uncertainty edges of every caller the Cone carries: the Primary half, then the
/// dependency half. Read by the coverage fold only; the solve and the context's
/// indexes read `primary().uncertainty_edges`.
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

Delete `compute_uncertainty_coverage_reasons` with its doc (`:3546-3616`). Tests deleted in
the same commit, because they pin the deleted code (I2's exception; listed again in §7):
- `check_oracle` (`:6331`) and its callers `callsite_local_scan_matches_the_fold_on_fixtures`
  (`:6477`) and `callsite_local_scan_matches_the_fold_on_cdo` (`:6568`, CDO-gated: the
  `cdo-gate` list loses one test). Their claim, "the scan equals the solve's fold", has no
  second side once the solve's fold is gone.
- `callsite_local_scan_matches_the_fold_under_a_roles_cap` (`:6630`): the same claim under a
  cap. No replacement (Q5): the new fold reads no solve and no cap.
- Kept unchanged except for the scan's new argument: `callsite_local_scan_hand_stated`
  (`:6422`), `snapshot_base_folds_the_scan_into_direct_coverage` (`:6597`),
  `snapshot_base_prints_no_capped_line` (`:6668`).

Doc comments that name the deleted function are rewritten in the same commit, so A10's grep
is clean: the scan's doc (`:3618-3638`, "Equal ... to ... the retired
`compute_uncertainty_coverage_reasons`": its argument that the solve never moves the four
kinds stays, pointing at `db_effect_solver.rs` instead) and the test module's doc (`:6132`).

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

**Fixture A** (T1, T1b, T2, T6-T8; inline tempdir, single-app unless stated): `Helper` makes
the ambiguous `O(L)` call (§0 item 7) and one `Modify` on a physical table, no Commit;
`Root` calls `Helper`, then `Commit()`; `Entry` calls `Root`. T6-T8 add an event publisher
and subscribers as stated.

| # | Test | Asserts | Break → failing assertion |
|---|---|---|---|
| T1 | **spans and d9** (row's proof; lib test over `build_detector_context` with `TRANSACTION_SPANS`) | preconditions, hand-stated: `rel.primary().uncertainty_edges` holds exactly one edge, `ambiguous-overload` from `Helper`; `Helper`'s direct reasons `== ["ambiguous-overload"]`; the explicit-Commit span's `routines_in_span == {Entry, Root}` and `pending_physical_tables.len() == 1`. Then: `Root` inherited `partial`; the span's `coverage_complete == false`; d9 emits exactly one finding for that span, in its incomplete-coverage wording (`d9.rs:77`) | delete the fold call in `det_cone_inputs` → `coverage_complete` is `true` and d9 emits nothing (the span has 1 table) |
| T1b | **digest and prove** (cli umbrella, production CLI): `alsem digest <ws> --routine Root` and `alsem prove <ws> Root may-commit`, both `--format json --deterministic` | the COMMIT effect's `transactionContext` is `unknown` in digest's output and in prove's `evidence`; digest's `coverage` key for `Root` is `partial` before and after the break (it is the snapshot's, which already folds) | same break → `transactionContext` known in both; the `coverage` key unchanged |
| T2 | **policy, both `onUnknown` outcomes** (cli umbrella, `alsem policy check <ws> --policy <file> --format json --deterministic`, the production pipeline incl. the RAW build `pipeline.rs:266`): two rules, both `when: {routine.name: Helper, capability.op: modify}`, `requireCoverage: complete`; A `onUnknown: fail-closed`, B `fail-open` | A: findings `==` one coverage finding on `Helper`; `routinesSkippedCoverage == 1`, `routinesMatched == 0`. B: no finding; `routinesSkippedCoverage == 1`, `routinesMatched == 0` (every other routine is skipped by `routine.name` before the gate) | delete the fold → A: one rule finding on `Helper`, skipped 0, matched 1; B: the same |
| T3 | **context == snapshot coverage per routine on r0** (row's acceptance; lib): every `tests/r0-corpus/` dir, single-app, every routine: `(direct_status, inherited_status, sorted reasons, sorted unknown_targets)` from the context (`det_cone_inputs` + `compose_cone_over_graph` over `rel.cone()`) equals the snapshot base's (`build_r3a3_source_only_base`) | equality over all dirs; precondition: at least 11 dirs have a routine with a four-kind reason (P5) | delete the fold in `det_cone_inputs` → fails on those dirs (it fails the same way on the base: the test is the bug's witness) |
| T4 | **dependency fixture, the context against its own Cone** (row's acceptance; lib): `write_fixture_dep_ambiguous` in `test_support.rs` (P6: dependency `Dep Rec Lib::Outer` → `Inner`, `Inner` makes the ambiguous `O(L)` call and a `Modify`; workspace `Ws Call::Go` calls `Outer`) | preconditions: `rel.dependency_uncertainty()` holds exactly one `ambiguous-overload` edge, from `Inner`; `rel.primary().uncertainty_edges` holds none of the four kinds; the fixture has no object run (no unresolved object-run source). Then: cross-app context: `Inner` direct `partial`; `Go` inherited `partial` with `Inner` in `unknown_targets`; equals an independent BFS over `rel.cone()` from `Go` (partial iff a reached row has a non-complete direct status) | fold `rel.primary().uncertainty_edges` only → `Go` is `complete` |
| T5 | **one fold, two cross-app producers** (lib): on T4's workspace, P-a's `coverage_in` equals P-b's `base.direct_coverage` | map equality | drop the fold call in `build_cross_app_base` → inequality on `Inner` |
| T6 | **fanout** (E4; cli): fixture A plus a publisher whose subscriber calls `Helper` | `events fanout --format json`: `capabilityComposition` `partial` for that event, exit 0 (default `warn`); `--coverage-policy strict`: the entry is dropped, stderr names it, exit 1; `--coverage-policy ignore`: the entry is kept with `complete` | delete the fold → `complete`; `strict` keeps the entry, exit 0 |
| T7 | **d35** (E4; cli): a subscriber reaching `Helper`, no Commit | one d35 finding, UNKNOWN wording, reasons text names `ambiguous-overload` | delete the fold → no finding |
| T8 | **d45** (E4; cli, opt-in so it names `--detector`): publisher/subscriber pair where the subscriber reaches `Helper` and writes a table | the finding's root cause carries `subscriberCoverage=partial` | delete the fold → `complete` |

T6-T8 can be one workspace with several runs (the `cli_d35_unknown_commit` pattern). T1, T3,
T4, T5 are lib tests. T4's fixture is shared with step 6's acceptance (Q7). d1 and d2 get no
test: their output cannot move (§3.2, the d1/d2 limit).

**The `O(L)` fixture is not compilable AL** (a Codeunit is neither a `Text` nor an
implementation of the interface). It works because our overload resolver is lenient; the
preconditions of T1 and T4 assert the ambiguous edge exists, so a future stricter resolver
fails them loudly instead of silently testing nothing. The fixture's doc says so.

`cli_d35_unknown_commit`: its variants use `object-run-unresolved`, a direct-coverage reason,
not a fold kind (P7), and the prototype left it green. Predicted unchanged, under F1 too.

## 6. Acceptance matrix

| # | Claim | Evidence | Pass rule | Feasible on the data? |
|---|---|---|---|---|
| A1 | Context == snapshot per routine, r0, single-app | T3 | all dirs equal; break fails | yes |
| A2 | Cross-app fold includes the dependency half, carried by the Cone | T4, T5 | green; breaks fail | yes (P6 shape) |
| A3 | Every reader that can move its output is listed and pinned by its use | §3.2; T1 (spans, d9), T1b (digest, prove), T2 (policy), T6 (fanout, all three `--coverage-policy` modes), T7 (d35), T8 (d45); d1/d2 by the stated limit | each break fails | yes |
| A4 | No existing golden or test outcome moves except the I2-exception deletions of §4.3 | `scripts/check-goldens`, `scripts/ci-steps task` on the head | green with no regen; a move is F5 | yes (prototype: 921/921, 2,241/2,241) |
| A5 | Snapshot-derived output byte-identical | single-app: the A4 goldens (r3a3, cli-b fingerprint/snapshot/diff/prove) and CDO `fingerprint` bytes, parent vs head; cross-app: CDO `digest` `coverage` key on every root of §8 step 4, parent vs head | 0 bytes moved in those keys (prove's and digest's `transactionContext` are §3.2 readers, not A5) | yes (prototype: fingerprint equal) |
| A6 | Resolution untouched | CDO `aldump --program-call-graph-stats` SHA, parent vs head | equal (`c032c8db…` on the parent) | yes |
| A7 | CDO ratchets | `bash scripts/cdo-gate` | exit 0 | yes |
| A8 | CDO moves triaged | `cdo-measure` comp; explicit d45; `--single-app`; the span enumeration and digest/prove runs (§8); `triage-findings` with §7's verdicts | each default-mode detector ≤ 30 % FP (§7's gating verdict) on its added findings at each scope, or F1/F2 applied | yes |
| A9 | Policy | CDO `policy check`, bundled policy, both universes: byte-identical to the parent. Proxy for "the owner's policy" (Q1): `ws-policy-custom`, both universes: deltas of findings and `routinesSkippedCoverage` reported; a seeded sample of 30 routines that went `complete` → `partial` and were dropped from `cust-coverage-gate` is checked to reach a four-kind edge | as stated | **partial**: CDO has no owner policy; the bundled one cannot move (§3.4) |
| A10 | L4-solve fold gone | `grep -rn compute_uncertainty_coverage_reasons src` | 0 hits (code and the two docs of §4.3) | yes |

## 7. False-positive and failure shapes

**Deleted tests (I2's exception, epic I2):** the four tests of §4.3 are deleted with the code
they pin; no test is rewritten.

**Verdicts (pre-stated, so they cannot drift).** Each added or changed item gets two verdicts.
- **Epistemic** (gating, per the row's > 30 % rule, for d35 UNKNOWN, d9's incomplete-span
  wording, d45's text, fanout's `partial`, and every changed `transactionContext`): is
  `partial` correct for the universe analysed? Decided per named uncertain call, by kind:
  - `interface-open-world`: open the interface's declaration. `Access = Internal` → closed
    world: the call is decidable from the app's own implementations, so the verdict is
    **false** (and the edge is a precision issue to file: an internal interface should not be
    open-world). `Public` (the default) → any extension may implement it → **true**.
  - `ambiguous-overload`: the AL compiler picked one overload in compiled code, so the call is
    statically decidable → **false**, and a resolver-gap issue to file.
  - `external-target`: the callee has no body in the universe analysed → **true**.
  - `member-not-found`: 0 on CDO; if it appears, **false** when the member exists in the
    closure's source, else **true**.
  An item is epistemically false only if EVERY uncertain call it names is false.
- **Hazard** (recorded, not gating): does the real code, dependency source included, have the
  hazard? d35: a Commit reachable from the subscriber. d9 and digest/prove: the span really
  reaches a routine the analysis could not finish. This tells the owner whether the honest
  `partial` is also a useful warning.
- d9 needs no Commit verdict (every d9 span is Commit-seeded, `d9.rs:36-39`); its epistemic
  verdict is the span's named reasons, by the rules above.

Shapes:
- **d35 "may be reachable" findings.** +41 primary, +83 all cross-app, +61 single-app
  (prototype); primary scope is two hubs, every one naming `interface-open-world`. The
  `Access` of the hubs' interfaces decides most of the verdict. F1.
- **Single-app `external-target`.** Every new single-app d35 finding names `external-target`
  (calls into dependency apps the mode excludes): epistemically true, likely hazard-false.
  `--single-app` is itself the opt-in mode (the default is cross-app since S8), so these are
  reported and sampled, not gated (Q8).
- **d9 info findings** and every changed span (+5 d9 cross-app, +10 single-app; the span
  enumeration of §8 step 4 covers spans d9 does not report).
- **d45 text** (opt-in): `subscriberCoverage` complete → partial on 65 findings; no count
  change.
- **d1/d2** walk more, output unchanged (§3.2's limit); a move is F5.
- **Policy fail-open silent drop** (§3.4): −182 cross-app under the custom policy. It is the
  declared meaning of `fail-open`, so it is not counted as FP; A9's sample checks that each
  dropped routine really reaches a four-kind edge.
- **Cross-app digest/prove disagreement until step 6** (5-A3): digest's `coverage` key can say
  `complete` while its `transactionContext` says `unknown`, for a root whose only four-kind
  edge is in dependency code (P2: 965 primary routines flip from the dependency half alone).
  Stated; closed by step 6's Cone snapshot.
- **A collided id** (11b): every row of the id folds the same reasons, and the projection
  takes the least complete, so the per-id value never becomes more complete.
- **An edge `from` with no row**: ignored, as today; 0 on CDO (P1).

## 8. Measurement plan

Non-speed (Decision 58): no bench. Binaries: release-fast `alsem` + `aldump` at the parent
(`f96619ec`, already in `/work/bench/160-5/parent/`) and at the head, copied out of `target/`.
The CDO path is read at run time (`jq -r .cdo_ws /work/alsem/.agent/config.json`). Each run in
the background, output to a log file; never piped through `tail`.

1. `cdo-measure run` both sides, then `comp`. Expected (prototype): fingerprint unchanged;
   analyze changes only in d35 and d9 (findings and their `detectorStats`), counts as §3.2.
2. d45 explicitly (opt-in): `alsem analyze <WS> --detector d45-event-transitive-table-exposure`
   at both scopes and `--single-app` (`/work/bench/160-5/run-d45.sh`). Expected: 150 → 150,
   text moves only.
3. `--single-app` analyze, both sides (`run-proto.sh`'s loop). Expected: d35 6 → 67, d9 27 → 37.
4. **Every changed span.** A CDO-gated `#[ignore]` probe test, kept outside the branch like P1,
   prints every transaction span's `(seed kind, commit routine, coverage_complete)` for the
   parent and the head, both universes. For each span that flipped: `alsem digest <WS>
   --routine <commit routine>` (both universes) and `alsem prove <WS> <commit routine>
   may-commit` (single-app), parent vs head; the diff must be only `transactionContext`
   (A5 reads the `coverage` key from the same digest runs).
5. `alsem events fanout <WS> --format json --deterministic`, both universes: per-event
   `capabilityComposition` diff; every flipped event must reach a four-kind edge (P14).
6. `alsem policy check`, bundled and `ws-policy-custom`, both universes (A9).
7. `aldump --program-call-graph-stats` SHA both sides (A6).
8. Triage (A8) with §7's verdicts: every added d35 and d9 finding of the default mode
   (46 at primary scope, 88 at all scope, the primary set included), the 28 flipped fanout
   events and the changed spans of step 4, root cause first (one hub, one verdict on its
   uncertain calls, then a check of each finding of the hub); the 65 d45 text changes the
   same way; single-app: a seeded sample of 30 (seed recorded). Rate per detector, per
   universe, per scope, epistemic and hazard side by side.
9. `bash scripts/cdo-gate` once, as the branch gate.

## 9. Amendments this spec asks the epic to take

- **5-A1** §6.3's reader list adds d9 (through spans), digest's and **prove's** tx context,
  and d45 (named as opt-in); names d1 (`d1_graph`) and d2 as readers whose output cannot move
  (§3.2's limit) and d34 as unable to move (reads `Yes` only).
- **5-A2** The row's "`policy check` with the owner's policy" is delivered with the proxy of
  Q1. Under the bundled policy (`requireCoverage: partial`) the fold cannot move the gate.
- **5-A3** Until step 6 the snapshot's fold input is its own cut graph's uncertainty (Primary
  callers, cross-app). The context and r3a5 fold both halves. Cross-app digest may show
  `coverage: complete` beside `transactionContext: unknown` for a root whose only four-kind
  edge is in dependency code. Step 6's acceptance closes it.
- **5-A4** M5 is decided: two fields kept, one chained accessor `uncertainty_all()`.
- **5-A5** §5.8's deletion of `compute_uncertainty_coverage_reasons` happens here, with the
  tests named in §4.3.
- **5-A6** E4's fixture list adds d9 (T1), digest and prove (T1b), the policy counters in both
  modes (T2), and fanout's `--coverage-policy` modes (T6).
- **5-A7** The row's golden list ("r3a5, cli-b digest/prove, cli-c-events, cli-c-policy, r4,
  cli-a, gate, `cli_d35_unknown_commit`") becomes "none predicted" (§0 item 1): the existing
  goldens do not witness the bug, and the new tests carry the evidence. The epic's "Goldens
  move in both universes" (§6.3) is withdrawn.
- **5-A8** The triage gate of the row (> 30 % FP) applies to the default (cross-app) mode at
  both scopes, with §7's epistemic verdict; `--single-app` moves are reported and sampled
  (Q8).

## 10. Fallbacks (each pre-stated; firing one needs no new review round)

- **F1 (d35 > 30 % epistemically false on its added findings, default mode, either scope).**
  (a) Root cause first: split the false ones by hub and uncertain call. Each systematic cause
  (an `Internal` interface emitted as open-world; an `ambiguous-overload` the compiler
  decides) is filed as its own issue against the resolver/graph with the hub as its repro,
  and recorded in the ledger's discoveries; it cannot be fixed in this step (A6).
  (b) Then the findings whose EVERY coverage reason is one of the four kinds (the step's new
  population) move to a new opt-in detector id
  `d35u-commit-may-be-reachable-in-event-subscriber`. Everything else in d35 stays default
  and byte-identical: the YES arm, and every UNKNOWN finding with at least one other reason
  (`object-run-unresolved`, #148; `cli_d35_unknown_commit` stays green). Pre-stated
  migrations: the id is registered in `OPT_IN_DETECTORS` with d35's substrate requirements;
  T7 asserts the finding under `--detector d35u-…` and its ABSENCE under the default
  selection and under `--detector d35`; a test asserts a mixed-reason UNKNOWN finding stays
  under d35 in the default selection; the CDO comp shows d35's default findings equal to the
  parent's plus the mixed-reason adds. The ledger records "back to default when (a)'s issues
  land". This splits one coverage's PRESENTATION across two ids; every reader still reads
  the one fold, so it does not re-create two coverage policies.
- **F2 (d9 > 30 % epistemically false on added findings, default mode).** The same split
  as F1 (a); then the d9 findings whose span's every incomplete reason is a fold kind move to
  an opt-in id the same way (`d9u-…`), with the same migration tests. (d1/d2 cannot move,
  §3.2.)
- **F3 (any detector has > 300 added findings in one mode and scope, or the head's P2 count
  exceeds 50 % of primary routines cross-app).** Triage a seeded random sample of 30 per
  detector (seed recorded) instead of every finding; the rule applies to the sample. (The
  prototype predicts neither: max 83, 18.4 %.)
- **Dependency-half attribution (was F4; no scope change).** If F1 or F2 fires, the ledger
  records how many false findings come from the dependency half only (re-run with the
  dependency half dropped from the scan input). The step still folds both halves: a known
  uncertainty is never dropped for a detector's precision; the detector-level F1/F2 carry it.
- **F5 (an existing golden or test outcome moves, other than §4.3's deletions).** Stop.
  Explain the move from the diff (the `golden-diff-triager` agent) before regenerating
  anything; a move traced to the fold in a reader §3.2 lists as moving is regenerated and
  recorded as a discovery against §0 item 1; any other move (d1/d2 included) is a bug to fix.
  No regen is a bless.

## 11. Risks

- **The move is honest but user-visible.** Some answers that said `complete` now say
  `partial`. The old `complete` was wrong; the triage is the gate.
- **Snapshot/context mismatch in cross-app until step 6** (5-A3).
- **The hubs.** Two routines explain all primary-scope d35 adds, all through
  `interface-open-world`. If their interfaces are `Internal`, d35 is 41/41 false at primary
  scope and F1 fires; it is written to fire without a new round.

## 12. Open questions for the spec panel (recommended answers)

1. **Q1. Which is "the owner's policy"?** *Recommended (round 1 agreed):* bundled policy
   byte-identical, plus `tests/fixtures/cli-c-policy/ws-policy-custom/al-sem.policy.yaml` as the
   `complete`-gate proxy, reported with A9's sample, not FP-gated.
2. **Q2. Should the snapshot fold the dependency half now?** *Recommended (round 1 agreed):
   no, wait for step 6* (5-A3). The cut graph has no dependency-internal edges, so folding now
   would reach only dependency routines called directly by primary code, a partial answer
   step 6 replaces.
3. **Q3. Opt-in for part of d35.** *Recommended (narrowed in round 1):* F1 (b), only the
   all-fold-kind UNKNOWN findings.
4. **Q4. Is folding `external-target` from dependency code right?** *Recommended (agreed):
   yes*; the callee has no body in the universe.
5. **Q5. Replace the deleted roles-cap test?** *Recommended (agreed): no.*
6. **Q6. Own use-pinning tests for d1 and d9?** *Resolved:* d9 in T1 (a real span and d9
   finding); d1/d2 by the stated limit of §3.2, since their output cannot move.
7. **Q7. T4's fixture is the one step 6 reuses?** *Recommended (agreed): yes*, with the
   leniency note of §5.
8. **Q8. Which mode does the > 30 % rule gate?** *Recommended (agreed):* the default
   (cross-app) mode at both scopes; `--single-app` sampled and reported with the same
   verdicts.
