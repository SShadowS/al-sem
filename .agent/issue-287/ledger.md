# #287 ledger

## Claim

```json
{"run_id":"20261025-211409-6d8487","issue":287,"attempt":1,"session":"claude-code-session:57653578-e111-4b15-8684-017371f816fa","branch":"issue/287-agent-discovery-l5-detector-con-a1","worktree":"/work/al-sem-issue-287-a1","body_hash":"b9a79cf71568fdb60a9e23c6e5278c98dfed5895a30e79ddf17cbd3412777cdc","reconciled":[]}
```

Base (make-worktree): `08c58d63077c22be3abe11e69d1d3611244ee57e`. Reviewers: gpt-6.1-sol (astra slot), local Opus (flash slot); attest with `--substitute astra=sol --substitute flash=opus`.

## Classification

**Bounded.** It changes one new index on the detector context and one reader (d35's message), and touches no resolver, no IR and no schema. The issue states both the capability and the acceptance.

## Assumption probes

| probe | result | evidence |
|-------|--------|----------|
| P1: the dependency half's uncertainty edges are kept apart from the Primary half, and the context indexes only the Primary half | confirmed | `src/engine/l4/exec_relation.rs:80-84` keeps `d.primary.uncertainty_edges` as `dependency_uncertainty`. `src/engine/l5/detector_context.rs:1624-1633` builds `uncertainty_edges_by_from` from `rel.primary().uncertainty_edges` only. |
| P2: the two halves' UNCERTAINTY edges are disjoint by `from`, so a caller's uncertainty edges sit in at most one half. (Plain edges are not disjoint: event dispatch can put a dependency publisher in the Primary graph.) | confirmed in the production build | The cross-app split partitions calls by caller, `partition(\|e\| dep_routine_ids.contains(&e.from))` (`src/engine/l4/capability_cone.rs:3996-4014`). `assemble` makes uncertainty edges only from call edges, with `from: ce.from`; event dispatch adds plain edges only (`src/engine/l4/combined_graph.rs:360-478`). `ExecRelation::new` itself takes any two slices and enforces nothing. `rel.uncertainty_all()` already chains both halves for the coverage fold, so the subscriber's coverage gap already counts its own dependency-half edges; only the message lacks them. |
| P6: size of the dependency half on CDO | small | The `rel.dependency_uncertainty` census bucket is 77,904 **bytes** shallow (`docs/measurements/2026-10-20-issue-160-step4.md:66-79`, column "head B"). That is about 541 edges at 144 B each (`UncertaintyEdge` = a `String` plus `Uncertainty`'s `String` and four `Option<String>`s). The new index copies those once, well under 1 MiB with their strings. |
| P7: d35's confidence cap for the unknown arm | reads the Primary-only view | `d35.rs:186-195` reads `ctx.uncertainty_view().values_of(&routine.id)`, which is built from the core summaries ∪ `uncertainty_edges_by_from` (Primary). A dependency subscriber's OWN dependency-half edges therefore do not reach that set. Its core-summary uncertainties still do (`detector_context.rs:1708-1725`). For A6d the set is empty. |
| P3: single-app mode has an empty dependency half | confirmed | `exec_relation.rs:79`: the dependency half is assembled only when `dependency` is non-empty, and `ExecRelation::single_app` passes none. |
| P4: d35 is the only reader that names own sites; every other `uncertainty_edges_by_from` reader stays on the Primary map | confirmed | `grep -rn uncertainty_edges_by_from src/engine/l5`: d35.rs:318, test_support.rs:455, detector_context_11b_tests.rs:401/664, plus the census (detector_context.rs:1875). The L4 solver's maps of the same name are its own (`summary_runner.rs`, `db_effect_solver.rs`) and are not touched. |
| P5: today's A6d output | reproduced by the issue's reproducer | `tests/cli/cli_d35_unknown_commit.rs:1791` asserts `partly analysed: HandleRaised (Codeunit 70011)` and no `unresolved:`. |

## Design

**Capability.** `DetectorArtifacts` gets a second index, `dependency_uncertainty_edges_by_from: HashMap<String, Vec<Uncertainty>>`. It has the same shape and the same build loop as `uncertainty_edges_by_from`, but it is built over `rel.dependency_uncertainty()` (`src/engine/l4/exec_relation.rs`). One private helper, `index_uncertainty_by_from(edges: &[UncertaintyEdge])`, holds the loop and builds both maps. It keeps today's per-key edge order and duplicates, so the Primary map's contents do not change. The new map is built eagerly, as its sibling is: on CDO it holds about 541 edges (P6), so it needs no substrate gate. Single-app it is empty (P3). `uncertainty_edges_by_from`, `uncertainties_by_node` and every other substrate keep their contents. No other detector reads the new field (P4). The three hand-built `DetectorArtifacts` literals in tests (`test_support.rs:442`, `d17.rs:466`, `d50.rs:688`) get the field, empty.

**Accessor.** `DetectorArtifacts::own_uncertainty_edges(&self, routine_id: &str) -> impl Iterator<Item = &Uncertainty>` yields the routine's Primary edges, then its dependency edges. Its doc states the invariant and cites the code (P2): in the production build, a caller's uncertainty edges sit in at most one half. So this is the routine's whole own-edge list in either mode. d35's `coverage_reasons` reads this accessor instead of `ctx.uncertainty_edges_by_from.get(&routine.id)`. Everything after that read is unchanged: the call-site-local kind filter, the dead-site check, the row's own site, `every_site_found`, `drop_self`, and the caps. A dependency subscriber therefore gets the same `; unresolved: <callee> (line n)` segment as a primary one, and drops itself from "partly analysed" under the same rule (#265). The same reasons string reaches the root cause, the evidence note and the fix hint (`d35.rs:186-220`). For A6d both reviewers traced `drop_self` to true: `S.NextKm` is the one live call-site-local site, it is on line 8, and no other row shares the id.

**Census.** Add a `context.dependency_uncertainty_edges_by_from` row next to `context.uncertainty_edges_by_from` (`detector_context.rs:1875`), in the same shape.

**Stated limit (new, P7).** The unknown arm's confidence cap still reads `uncertainty_view()`, which is built from the core summaries and the Primary-only map. A dependency subscriber's message now names its own site, but its own dependency-half edges do not contribute to its confidence. Its core-summary uncertainties still do. For A6d that leaves the set empty and the confidence uncapped (`possible`). Widening that view would change every detector that reads it, which the acceptance rules out. This limit goes in d35's module doc and in the CHANGELOG entry, and is filed as discovery D1.

**Docs.**
- d35's module doc: replace "Primary subscribers only: … (stated limit)" (`d35.rs:38-40`) with the new reach and the P7 limit. Fix line 4, which says candidates are "primary+body+!parseIncomplete only"; the loop does not filter by role.
- `exec_relation.rs:38-41`, `:132-134` and `:139-140`: the context also indexes the dependency half now, in its own map.
- `detector_context.rs:858`: the `uncertainty_edges_by_from` doc says it is the Primary half only, and points to the new field.
- `cli_d35_unknown_commit.rs`: the test `dependency_subscriber_keeps_todays_text` is renamed `dependency_subscriber_names_its_own_sites`, and its doc comment is rewritten (A1).
- `cli_batch_contract.rs:145-150` still holds as written, so it is left alone.
- CHANGELOG `[Unreleased]` `### Fixed` gets one entry. It says that it lifts the #265 entry's stated limit (released section, `CHANGELOG.md:432-447`, left as is) and carries the P7 limit.

**Fallback (pre-stated), narrowed.** A1's two assertions are mandatory: the `unresolved:` segment, and HandleRaised absent from "partly analysed". If `drop_self` comes out false when run, the implementer finds the cause in the actual model and edges, from the routine's rows and its own-edge list. If the cause is a defect in this change, it is fixed. If it is a rule of #265 that applies as designed, it goes back to the panel as a finding with that evidence. It does not ship as "partly delivered".

**Not changed.** Finding ids, fingerprints, levels, severities, confidence caps, and the primary subscriber's text. The single-app output. The L4 solver.

## Acceptance matrix

| # | Acceptance item | Proof |
|---|-----------------|-------|
| A1 | The dependency subscriber's message names its own site (callee and line) and no longer lists itself | `tests/cli/cli_d35_unknown_commit.rs::dependency_subscriber_names_its_own_sites` (renamed from `dependency_subscriber_keeps_todays_text`, updated deliberately). It keeps the precondition of exactly one d35 finding on HandleRaised. It runs in both the default selection and `--detector d35`, and in each it asserts:<br>- the fix hint, the root cause and the evidence note each carry one exact reasons string ending `; unresolved: S.NextKm (line 8)`. The implementer fixes its leading coverage-reason words ONCE, from the first green run, and writes the whole string as a literal in the test; it is then matched exactly;<br>- none of them contains `partly analysed`;<br>- the id is `d35/<HandleRaised id>/transitive` and the severity is `medium`;<br>- the WHOLE `confidence` object (level, `cappedBy`, and any evidence it carries) equals the value recorded from a run on base 08c58d63, written as a literal;<br>- the fingerprint equals the base value, and is equal across the two selections.<br>Discrimination: point d35 back at `uncertainty_edges_by_from` only, and the test fails. |
| A2 | Primary-subscriber output unchanged | The existing #265/#288 d35 CLI tests (`cli_d35_unknown_commit.rs`) stay green unchanged. Every golden family stays green; a golden may move only where every changed line is a dependency subscriber's d35 reasons text, and each such line is triaged. `--scope primary` and single-app mode are covered by these indirectly: the new map is empty single-app (A3 pins that), and primary subscribers read the same map as today. |
| A3 | No detector reading `uncertainty_edges_by_from` changes (its contents are kept); the new map holds only the dependency half | Extend the existing `capability_cone.rs::step4_dependency_uncertainty_stays_out_of_primary` rather than add a new fixture. Add a workspace routine with its own uncertainty edge to its fixture if it has none, so both halves are non-empty. New checks on the cross-app context:<br>- (3) `dependency_uncertainty_edges_by_from` has exactly the key DepCaller, with exactly its kept edge's uncertainty, and `own_uncertainty_edges(DepCaller)` yields exactly that;<br>- (4) `uncertainty_edges_by_from` equals the Primary half grouped by `from` in edge order, with values and multiplicity exact, so the workspace routine's edge is there and DepCaller is absent;<br>- (5) a single-app context over the same workspace has an empty `dependency_uncertainty_edges_by_from`.<br>Discrimination: building the new map from the Primary half fails (3); dropping the Primary map's edges fails (4). |
| A4 | Capability: detectors read the dependency half via `DetectorContext`, keyed by `from` | The `dependency_uncertainty_edges_by_from` field plus `own_uncertainty_edges`, pinned by A3. |
| A5 | Census row present | In A1's test file: one `ALSEM_RETAINED_CENSUS=1` run on A6d's workspace prints a `context.dependency_uncertainty_edges_by_from` bucket with non-zero bytes. |

## Timeline

- 2026-10-25 21:14: claim ok (attempt 1). make-worktree ok, base 08c58d63. Loop 1/15.
- Classified bounded; probes P1–P5 recorded; Design written.
- Spec panel r1 (spec_rounds 1/5): sol approve-with-fixes (7), opus approve-with-fixes (14). Merged into R1–R13; R4 refuted (P6 bytes, not edges); R3 a stated limit + D1.
- Spec panel r2 (2/5): opus accepted all; sol re-raised R3 and R5 (minor). Wording narrowed; A1 compares the whole confidence object.
- Spec panel r3 (3/5): both accepted R3 and R5; verdict approve from both. **Converged**, 13 entries accepted by both. Opus's optional r3 note (take the reasons string's leading words from the base run) goes into the implementer brief; no Design change.
- Implementer step 1: base literals recorded on 08c58d63 by a temporary `eprintln!` in `dependency_subscriber_keeps_todays_text` (added with Edit, run, reverted with Edit; `git status` clean after): HandleRaised's d35 finding, default selection and `--detector d35-commit-in-event-subscriber` alike: confidence `{"level":"possible"}`, fingerprint `84abf5ea8c006512`, reasons leading words `interface-open-world`.
- Implementer steps 2-3 (red): A1 FAIL (fixHint `…(interface-open-world; partly analysed: HandleRaised (Codeunit 70011))…`, no `unresolved:`); A3 check (4) PASS against today's map alone, then (3)/(5) compile red (E0609 no field `dependency_uncertainty_edges_by_from`, E0599 no method `own_uncertainty_edges`); A5 FAIL (census ran, 77 lines, bucket missing). Steps 4-5: all three PASS; 28/28 d35 CLI tests pass.
- Implementer step 7: committed 1ae757f5. `task-affected` (whole suite, unmapped path): fmt, clippy-dev, gen-syntax green; tests stopped fail-fast at the known pre-existing red `cache_version_grammar_tracks_the_linked_grammar` (shared grammar clone at v4.5.0, base expects 4.4.1; conductor note). `cargo nextest run --workspace --no-fail-fast`: 4475/4476 passed, only that test failed. `scripts/check-goldens` (no regen): 1306/1306 passed, no golden moved. The pre-commit hook did not run (no `core.hooksPath` in this clone), so check-goldens was run by hand. check-diff: empty reasons.
- Rebased onto d53b8ff0 (grammar v4.5.0, #346). CHANGELOG `[Unreleased]` conflict: kept both entries, `### Changed` (grammar) then `### Fixed` (#287). rebase_regate 1/1 used. 1ae757f5 became 5152d6ac.
- Task gate on 5152d6ac: exit 0, 4476/4476 passed (`.agent/runs/20261025-211409-6d8487/logs/ci-steps-task/20261025T215023-7fa2b30e.log`). Pushed; draft PR #348.
- Final panel r1 (final_rounds 1/5):
  - sol: approve with fixes, 4 minor.
  - opus: approve with fixes. F2 important: D1 not written. F1, F3, F4 minor.
  - code-review (high, built-in skill): 7 findings. CR1 = R3's ratified limit, refuted with D1. CR3 = R4's memory point, refuted (P6). The rest are fixed.
  - Merged into FC1–FC11. The fixes are e79fb97b (implementer, the same agent via SendMessage): task-affected exit 0 (whole suite 4476/4476), and a reverse-mutation proof for C8/C9.
- Final panel r2 (2/5): opus approve (all accepted). sol re-raised FC2 (minor): the wording overstated the limit. d9c46305 narrows the CHANGELOG and d35's doc; D1's symptom has the same narrowing.
- Rebased cleanly onto 21cd0f1d (#347, CLAUDE.md only). Range-diff: all three branch commits `=`, no extra round needed. 5152d6ac → f57fc9af, e79fb97b → 91951ed2, plus d9c46305. Register commit anchors re-pointed.
- Full gates on d9c46305, all supervised, exit 0, queued 0 s. Peer alsem-second paused its heavy work for cdo-gate.
- Final panel r3 (3/5): both accepted FC2, FC5 and FC8; both verdicts approve. **Converged**: 24 entries, all accepted by both.

## Discrimination proofs

| test | mutation | fail output | pass output | commit |
|------|----------|-------------|-------------|--------|
| `cli_d35_unknown_commit::dependency_subscriber_names_its_own_sites` (A1) | d35.rs: `for u in ctx.uncertainty_edges_by_from.get(&routine.id).into_iter().flatten()` instead of `own_uncertainty_edges` | FAIL: `A1 None: fixHint` left `…(interface-open-world; partly analysed: HandleRaised (Codeunit 70011))…` right `…(interface-open-world; unresolved: S.NextKm (line 8))…` | PASS after revert | 1ae757f5 |
| `capability_cone::step4_tests::step4_dependency_uncertainty_stays_out_of_primary` (A3 (3)) | detector_context.rs: `dependency_uncertainty_edges_by_from = index_uncertainty_by_from(&graph.uncertainty_edges)` (Primary half) | FAIL: `(3) … want {DepCaller: [unresolved-call]}, got {WsUnc: [unresolved-call]}`; `(3) own_uncertainty_edges(DepCaller): … got []`; also `(5) single-app dependency_uncertainty_edges_by_from: {…}` | PASS after revert | 1ae757f5 |
| same test (A3 (4)) | detector_context.rs: `uncertainty_edges_by_from = index_uncertainty_by_from(&[])` | FAIL: `(4) context uncertainty_edges_by_from: want {WsUnc: […]}, got {}`; `(5) precondition: the single-app Primary map holds WsUnc: {}` | PASS after revert | 1ae757f5 |
| `cli_d35_unknown_commit::dependency_uncertainty_index_has_a_census_row` (A5) | detector_context.rs: the `context.dependency_uncertainty_edges_by_from` `emit(…)` deleted | FAIL: `A5: one context.dependency_uncertainty_edges_by_from bucket with bytes > 0, got []` | PASS after revert | 1ae757f5 |
| `capability_cone::step4_tests::step4_dependency_uncertainty_stays_out_of_primary` (A3 (3), (4), accessor, final r1 C8/C9) | detector_context.rs `index_uncertainty_by_from`: `by_from.values_mut().for_each(\|v\| v.reverse());` before the return | FAIL exit 100: `(3) context dependency_uncertainty_edges_by_from: want …cs0…`, `(3) own_uncertainty_edges(DepCaller)`, `(4) context uncertainty_edges_by_from`, `(4) own_uncertainty_edges(WsUnc)` | PASS after revert (grep count of `v.reverse()` 0) | e79fb97b |

Limit (C8): two uncertainties under one caller that are byte-equal cannot come from AL source, because each call site has its own `callsite_id`. A dedup mutation is therefore not discriminated, and no hand-built duplicate was added.

The first four proofs above ran on `1ae757f5` (base 08c58d63). The rebase onto d53b8ff0 made that commit `5152d6ac`. `git diff 1ae757f5 5152d6ac` touches only the grammar-bump files and the CHANGELOG context, and the branch's own code and tests are unchanged, so the proofs carry over (final r1, Opus F3).

## Findings register summary

There are 24 entries, all accepted by sol (astra slot) and opus (flash slot). Attest with `--substitute astra=sol --substitute flash=opus`.
- Spec R1–R13: R4 refuted, the rest fixed.
- Final FC1–FC11: FC8 and FC9 refuted, the rest fixed.

## Base/Head SHAs

B = `21cd0f1de18b5226348eac10d153698f5c579c72`. H = `d9c463059072840a1f7eeede7329a3ec7a47773e`.

## Gate results

| gate | exit | log |
|------|------|-----|
| ci-steps-all | 0 | `.agent/runs/20261025-211409-6d8487/logs/ci-steps-all/20261025T220859-b7b47803.log` (4476/4476) |
| check-goldens-coverage | 0 | `.agent/runs/20261025-211409-6d8487/logs/check-goldens-coverage/20261025T221632-8562edbf.log` |
| check-goldens | 0 | `.agent/runs/20261025-211409-6d8487/logs/check-goldens/20261025T221633-d788d50c.log` (tree-stamp: this exact tree passed `ci-steps test`) |
| cdo-gate | 0 | `.agent/runs/20261025-211409-6d8487/logs/cdo-gate/20261025T221648-8100ab10.log` (ENFORCE_CDO_WS=1, lib 204 passed) |

Toolchain: rustc 1.96.0. Grammar: tree-sitter-al v4.5.0, `91a5db62`. CDO_WS is the pinned baseline, enforced by the gate's own closure stamp. No golden moved.

## Discoveries

```json
[
  {
    "subsystem": "l5 detector context (uncertainty view for dependency routines)",
    "locator": "src/engine/l5/detector_context.rs (uncertainties_by_node: core summaries ∪ uncertainty_edges_by_from); src/engine/l5/detectors/d35.rs unknown arm (confidence from uncertainty_view)",
    "symptom": "In the default cross-app mode a dependency routine's own uncertainty edges (rel.dependency_uncertainty()) never reach uncertainty_view(), and its core summary is solved over the Primary half, where it has no out-edges unless it publishes an event. So a dependency subscriber's dependency-half call-edge gaps (own and callee) never cap its d35 unknown-arm confidence; only core-summary uncertainties (e.g. a parse-incomplete row sharing its id) do. For the A6d fixture that leaves it uncapped: after #287 its message names `unresolved: S.NextKm (line 8)`, yet confidence is {\"level\":\"possible\"} with no cappedBy, while the same code in the workspace gets cappedBy [\"dynamic-dispatch\"]. Every path-walker reader of a dependency routine's uncertainty set sees the same gap.",
    "kind": "limitation",
    "origin_issue": 287,
    "reproducer": "tests/cli/cli_d35_unknown_commit.rs dependency_subscriber_names_its_own_sites: HandleRaised's confidence is pinned at {\"level\":\"possible\"} although its own interface-open-world site is named in the text.",
    "pre_existing": true,
    "capability": "The uncertainty view (or a dependency-aware variant of it) carries a dependency routine's own dependency-half uncertainty edges, so confidence caps agree with the message for dependency subscribers, without changing the result for primary routines.",
    "acceptance": "HandleRaised in the A6d workspace at --scope all gets the same confidence (cappedBy and evidence) as an identical workspace subscriber; no primary-routine finding's confidence changes on the corpus or on CDO."
  }
]
```

## Not delivered
