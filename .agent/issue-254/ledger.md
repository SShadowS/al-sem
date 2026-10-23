# Issue #254 — digest: test coverage of changed routines

## Claim

```json
{"run_id":"20261023-180348-ce43e4","issue":254,"attempt":1,"session":"claude-code-session:57653578-e111-4b15-8684-017371f816fa","branch":"issue/254-digest-diff-test-coverage-of-ch-a1","worktree":"/work/al-sem-issue-254-a1","body_hash":"33201de80f51cfbc0e14c4723388d53f10c51a3877ecf23f0b3d0544f3fa6256","title":"digest --diff: test coverage of changed routines (reachable from [Test] via the resolved call graph)","reconciled":[]}
```

Issue body hash (claim `body_hash`): `33201de80f51cfbc0e14c4723388d53f10c51a3877ecf23f0b3d0544f3fa6256`.
Base (`make-worktree`): `7c35891a63ceb87e20e85e31131d2f79775a9ea2`.
Reviewers (operator instruction, 2026-10-23): `gpt-6.1-sol` stands in for the astra slot, Opus for the flash slot; attest with `--substitute astra=sol --substitute flash=opus`. Marks are stored as `reviews: {sol: …, opus: …}`.

## Classification

**Bounded.** One subsystem (`l5` digest), one new output block behind one flag, a clear spec in the issue (rules + 7 acceptance items). It reuses what the digest already builds: the snapshot's typed call edges and call-site resolution ledger, the model's routine attributes and object `Subtype`, and the event graph. No new engine resolution, no schema change elsewhere.

## Assumption probes

| probe | result | evidence |
|-------|--------|----------|
| P1 digest entries carry the changed routine's stableId | holds | `DigestEntryFull` (`src/engine/l5/digest_cli.rs:562-570`), projected as `routine.stableId` |
| P2 the engine can tell a real `[Test]` procedure | holds, from the attribute | `ModelRoutine.attributes_parsed` (the source `root_classification.rs:268-274` reads). NOT the snapshot `root_classifications`: `roots.config.json` can add kinds there (`root_classification.rs:593-668`) and rows can repeat (sol #5) |
| P3 the engine knows test codeunits | holds | `ModelObject.object_subtype` (`program/model/workspace.rs:57-64`) |
| P4 the snapshot's typed edges carry resolved calls with kinds | holds | `CapabilitySnapshot.typed_edges` (`snapshot.rs:890`), built at `combined_graph.rs:621-917`: `direct-call`, `variable-typed-call`, `implicit-trigger`, `interface-dispatch` (one edge per resolved candidate, `combined_graph.rs:745-786`), `object-run-resolved`, `object-run-unresolved` (no `to`), `event-dispatch` (from the publisher routine, `combined_graph.rs:869-891`) |
| P5 the snapshot has NO uncertainty edges | holds (design corrected) | `CapabilitySnapshot` fields `snapshot.rs:887-907`; uncertainty edges live only in the combined graph. Unresolved sites are visible through the call-site resolution ledger `callsite_resolutions` (`snapshot.rs:782-796`, built `snapshot.rs:2072-2365`), statuses `resolved`, `builtin`, `polymorphic`, `ambiguous`, `unresolved-member`, `external`, `unfetched-dependency`, `dynamic-target`, `unresolved-receiver-type`. `unresolved_cone` already walks typed edges + this ledger (`unresolved_cone.rs:67-160`) |
| P6 the digest graph holds dependency-internal CALLS | no | the snapshot base is built from the primary-only call list ("legacy cut", `capability_cone.rs:4502-4513`). Event dispatch is kept for every endpoint (`capability_cone.rs:3685-3690`, `combined_graph.rs:878-903`), so a test → dependency publisher → workspace subscriber edge that IS present is followed |
| P7 platform-event subscribers have an `event-dispatch` edge | no | platform events have `publisher_routine_id: None` (`program/model/events.rs:271-276, 499-503`); typed `event-dispatch` needs a model publisher routine (`combined_graph.rs:874-882`) |
| P8 null survives the digest serializer | no | `sort_and_drop_nulls` drops every `null` (`gate/format_json.rs:53-70`); `ratio: null` needs its own path |
| P9 CentralGauge's refapp is available here | no | not on this host; acceptance is proved on fixtures |

## Design

Spec = this section (bounded issue). Round 2 rewrote it after round 1 (sol 12 findings, opus 20).

**Surface.** New flag `alsem digest … --tests` (the issue's own example spelling, which the consumer already expects). When set, the digest JSON payload gains a `testCoverage` block. Without it the document is byte-identical to today. The issue allows either a flag or always-on; the flag keeps every existing digest consumer and golden unchanged, and CentralGauge adds one argument. The block is computed for any changed input (`--diff`, `--changed`, `--routine`). Human output adds one line only with `--tests`: `test coverage: 11 of 17 changed routines reached; 12 [Test] procedures considered (partial: dynamic-target, external)`.

**Inputs (already built by `digest_pipeline_with`).** The snapshot `snap` (typed edges, `callsite_resolutions`, identity/display tables), the model `resolved` (routine attributes and owning objects, object subtype, the event graph `resolved.events`, `primary_guid_of(resolved)`), and the digest entries.

**Test roots.** Every routine of the PRIMARY app (`Model::routine_role` / `primary_guid_of`, `digest_cli.rs:364`) whose parsed attributes include `Test` (case-insensitive), deduplicated by stable id. `testProceduresConsidered` is their count. Dependency routines are never roots, even with `[Test]`.

**Changed routines.** The distinct stable ids of the digest entries, minus (a) every test root and (b) every routine in a primary-app codeunit with `Subtype = Test` — exactly the issue's rule. `changedRoutines` is the count after these removals; `excludedTestCode: n` reports how many were removed. Declaration-only entries (interface and ControlAddIn method declarations, `digest_cli.rs:837-861`) STAY in the denominator, as the issue defines it; they are never graph nodes, so they are always `uncovered` with reason `declaration-only` (see §Why `uncovered`), which tells the consumer why.

**Followed edges.** Every typed edge with a routine `to`, whatever the endpoint's app: `direct-call`, `variable-typed-call`, `implicit-trigger`, `object-run-resolved`, `interface-dispatch` (each resolved candidate is a known target), `event-dispatch` (publisher → subscriber). Plus one edge the snapshot lacks: test → each handler named in its `[HandlerFunctions('A,B')]` (resolved within the test's own object, as `impact_graph/facts.rs:210` reads it), since the platform runs the handler during that test. Not followed: anything without a routine `to`, and every ledger row other than `resolved`/`builtin`/the candidates of `polymorphic`.

**Incomplete traversal.** For each routine a test reaches (itself included), its `callsite_resolutions` rows are bucketed by status:

| status | bucket | reason code |
|---|---|---|
| `resolved`, `builtin` | followed | — |
| `polymorphic` with candidates, no unresolved candidates | followed | — |
| `polymorphic` with no candidates or with unresolved/dependency implementers | incomplete | `interface-open-world` |
| `ambiguous` (closed overload set, no typed edge) | incomplete (not followed: following every candidate over-claims) | `ambiguous` |
| `dynamic-target`, `unresolved-receiver-type`, `unresolved-member` | incomplete | the status itself |
| `external`, `unfetched-dependency` | dependency boundary | the status itself |

plus, per reached routine: an `object-run-unresolved` typed edge from it → incomplete, reason `object-run-unresolved`; a `TestPage`/`TestRequestPage` parameter, local, or global of its object (any reached routine — a test, a helper or a handler; globals as `impact_graph/facts.rs:196-202` reads them) → incomplete, reason `test-page` (the snapshot has no edge for what a TestPage drives); its snapshot coverage record carrying `parse-incomplete` → incomplete, reason `parse-incomplete` (its missing call sites have no ledger rows, `capability_cone.rs:527-536`); NO coverage record at all → incomplete, reason `no-coverage-record`; its coverage record carrying `opaque-dependency` → dependency boundary, reason `opaque-dependency`; and the routine being outside the primary app → dependency boundary, reason `dependency-code` (the snapshot holds no dependency-internal calls, P6, so its body is a traversal boundary even when the dependency has source). Following the issue ("opaque callee" does not count as reachability, and a test with such traversal is reported), a dependency boundary also makes the block partial; it is counted apart so the consumer can tell the two.

The block's `coverage` is `{ "status": "complete" | "partial", "reasons": [sorted distinct codes], "testsWithUnresolvedTraversal": n, "testsCallingDependencyCode": n }` (same `status`/`reasons` shape as each entry's `coverage`). `testsCallingDependencyCode` counts tests whose reach has any dependency-boundary item: a ledger row, an `opaque-dependency` record, or a reached non-primary routine (cross-app shape; single-app has only the ledger shape). Every dependency boundary also puts its reason in `reasons` and makes `status` `partial`. `testsWithUnresolvedTraversal` counts tests with an `incomplete` row. In single-app mode a call to a dependency TABLE procedure on a record receiver shows as `unresolved-receiver-type` (`program/model/program_calls.rs:73-91`) and so lands in "unresolved": stated, not hidden.

**Why `uncovered`.** Each `uncovered[]` item carries `reason`, first match wins: `declaration-only` (an interface/ControlAddIn method declaration: no body to execute), `platform-event-subscriber` (it subscribes to a platform event, no publisher routine exists), `dependency-event-subscriber` (it subscribes to an event whose publisher is in a dependency and no test reaches that publisher), or `no-path`. Derived from `resolved.events` (the subscriber's event's publisher reference). CentralGauge solutions subscribe to base-app and platform events, so this is what tells a blind spot from untested code.

**Search.** For each test root in ascending stable-id order, a BFS over states `(routine, crossedEvent)` with neighbour lists sorted by stable id. A routine's witness is the first test that reaches it (the lowest stable id). Its `viaEvent` = `dist_noEvent(witness, n) > dist(witness, n)`: true when the shortest path from the witness must cross an `event-dispatch` edge; an equal-length event-free path wins. Each test's full reach is needed for the per-test counters, so the cost is O(T × (V + E)) over the snapshot graph, which in cross-app mode includes the dependency routines the model holds. The CDO timing check below measures it.

**Output.** `covered` and `uncovered` sorted by stable id, disjoint, together exactly `changedRoutines` entries. `covered[]`: `stableId`, `display`, `reachedFrom: [one id]`, `viaEvent`. `uncovered[]`: `stableId`, `display`, `objectDisplay`, `reason`, and `anchor {file, line}`: the entry's own `routine_anchor`, unchanged (the last touched declaration, `digest_cli.rs:823-832`), OMITTED when the entry has none. `ratio = covered / changedRoutines` as a JSON number, and `null` when `changedRoutines` is 0. Because the document serializer drops every `null` (P8), `project_digest_document` takes the block as `Option<serde_json::Value>` and inserts it into the payload AFTER `sort_and_drop_nulls` and before pretty-printing (a small helper in `gate/format_json.rs` that prints an already-normalised value; serde_json's map without `preserve_order` keeps keys sorted), so `ratio: null` survives; every other optional field is absent-when-missing as elsewhere. `schemaVersion` stays `1.6.0` (the block is opt-in and additive; a bump would move every digest golden with the flag off); its history comment (`digest_cli.rs:1407-1413`) gets a line naming `payload.testCoverage` behind `--tests`.

**Code placement.** A new module `src/engine/l5/test_coverage.rs` in two layers. (1) A small read-only input `CoverageGraph` (test roots, test-code ids, followed edges with an `is_event` bit, per-routine bucketed rows, non-primary routine ids, the uncovered-reason facts, display/anchor lookups) built by ONE adapter `CoverageGraph::from_snapshot(snap, resolved)`, the only code that knows the snapshot's shape. (2) A pure `compute_test_coverage(&CoverageGraph, changed) -> TestCoverage` with unit tests that build `CoverageGraph` by hand. #160 step 6 Task 2b-2 will later move digest onto the cone graph and may change the snapshot; then only the adapter changes, and the fixture tests pinning the exact sets fail loudly if the answer moves. `digest_cli.rs` gets a few lines: build the block before `drop(snap)` when the flag is set, and pass it to `project_digest_document` (§Output). The flag goes through `DigestRequest` and the `alsem digest` clap args (`src/bin/alsem.rs`). CHANGELOG `Added`. **Out of bounds:** `digest.rs` effect/witness code, `ordering_engine.rs`, `fingerprint_query.rs` (#160 step 6, another session): not edited; the snapshot is only read.

**Why not the other graphs.** `alsem graph` (`impact_graph`) is a may-execute graph: it adds lifecycle, declared-runnable, test-runner and Open hub edges and uses hashed ids joined by location (`impact_graph/mod.rs:22-75`). Counting its paths would count exactly the open-world paths the issue excludes, so it is not reused (only its `[HandlerFunctions]` reading rule is). bc-dev-mcp's test selection (built on it) can therefore select a test that `digest --tests` does not count as covering; that is intended (selection over-approximates, coverage must not). `view.rel().cone()` holds dependency-internal calls too and would remove the P6 limit where dependency SOURCE exists (CDO); it makes no difference for CentralGauge's symbol-only dependencies, and #160 step 6 moves digest onto it anyway, at which point the adapter follows.

**Stated limits.** (a) A workspace subscriber to an event raised only inside dependency code (P6) or to a platform event (P7) is uncovered, with its `reason`. (b) Pages driven through `TestPage` are not followed (`test-page` reason). (c) A test codeunit's `OnRun` is not a root. (d) Test-library codeunits with a normal subtype, and `Subtype = TestRunner` codeunits, count as production code. (e) Cycles end at the visited set.

**Measurement plan.** None beyond the tests: the block is off by default and the default path is unchanged. One timing check, recorded in Gate results with `testProceduresConsidered` (T), the summed per-test reached-routine count and the wall time with and without `--tests`. The pinned CDO workspace `Cloud` has NO `[Test]` procedures (grep: 0 `Subtype = Test` codeunits, 0 `[Test]`), but its sibling app `/work/src/cdo-baseline/Test` has 1844 `[Test]` procedures calling `Cloud` as a dependency (read-only use, nothing in the baseline changes). The check runs `alsem digest /work/src/cdo-baseline/Test --changed <one large test-helper file> --tests` (every test is a root; cross-app reach into `Cloud`). Fallback if that workspace does not build (its dependencies are not resolvable here): a throwaway copy of `Cloud` under `/work/bench/254/` plus one generated test codeunit with 200 `[Test]` procedures each calling a different public `Cloud` procedure; if that fails too, run it on `tests/fixtures/test-selection-graph/` only, state in Not delivered that the cost at scale is unmeasured, and file a discovery for a scale measurement.

## Acceptance matrix

Fixtures live under `tests/fixtures/` (not `tests/r0-corpus/`, so no r4/r2c/l2 golden moves). The main one is the existing `tests/fixtures/test-selection-graph/` (direct call, `Codeunit.Run` chain, event-only subscriber, interface dispatch, an unreached routine `LegacyExport.ExportLine`, dynamic sites, a TestPage, a handler, a platform-event subscriber), driven with `--routine` selectors over its production routines; plus a new `tests/fixtures/digest-test-coverage-no-tests/` (one production codeunit, no tests). Tests: a new member of the `cli` umbrella, `tests/cli/cli_digest_test_coverage.rs`, through the `alsem` binary; unit tests in `test_coverage.rs`. Each test hand-states the exact expected `covered` / `uncovered` sets.

| # | Acceptance | Proof |
|---|------------|-------|
| 1 | direct call from `[Test]` → covered | CLI: a routine a test calls directly is in `covered`, `viaEvent: false` |
| 2 | subscriber covered when a test reaches the publisher | CLI: the `Invoice.al` subscriber reached only through its event → `covered`, `viaEvent: true`; unit: raiser → publisher → subscriber |
| 3 | reachable only through unresolved/opaque → uncovered, traversal incomplete | CLI: a routine reached only through a `Dynamic.al` site → `uncovered` `no-path`, block `partial` with `dynamic-target` (or the fixture's actual ledger status) in `reasons`, `testsWithUnresolvedTraversal ≥ 1`; unit: a graph whose only incompleteness is one ledger row flips `status` (discriminates, unlike a block that is always partial) |
| 4 | test routines and test codeunits not counted | CLI: selecting a `[Test]` procedure and a test-codeunit helper → neither in `covered`/`uncovered`, `excludedTestCode` = 2; unit: a configured-kind-only "test" (no attribute) is NOT a root |
| 5 | covered + uncovered = changedRoutines, no duplicates, sorted | unit + CLI assertion on every run; a declaration-only entry stays in the count, `uncovered` with reason `declaration-only` |
| 6 | byte-identical with `--deterministic` | CLI: two runs, byte compare, on a selection where several tests reach the same routine (witness ordering exercised) |
| 7 | fixtures: direct, two-hop, pub→sub, uncovered, no tests | the above + two-hop (`RecalcNightly` → `OnRun` → `ApplyDecay`), uncovered `ExportLine` (`no-path`), the no-tests fixture (`testProceduresConsidered: 0`, all uncovered, `ratio` = 0 with changed > 0); and `ratio: null` (key present) when only test code is selected |

The consumer's own shape: one CLI case runs `alsem digest <test-selection-graph> --diff <a unified diff touching two production routines and one test> --tests --format json --deterministic` and asserts the exact sets (the diff → changed-roots path, not only `--routine`).

Extra unit cases (sol #9, round 2): interface dispatch with two known candidates plus an unresolved-implementer remainder (both candidates covered, block `partial` with `interface-open-world`); a test reaching a dependency publisher whose subscriber is in the workspace (covered) vs a workspace subscriber to a dependency event no test reaches (uncovered `dependency-event-subscriber`); a reached routine with a `parse-incomplete` record and one with no record (both `incomplete`); a `TestPage` parameter on a reached handler and a `TestPage` global only (both reason `test-page`); a reached non-primary routine (`dependency-code`, counted in `testsCallingDependencyCode`, not in `testsWithUnresolvedTraversal`); lowest-id witness when a higher-id test has a shorter path; equal-length event vs non-event path (`viaEvent: false`); a shorter event path (`viaEvent: true`); a cycle; shuffled adjacency gives the same output; duplicate root rows; a dependency-boundary row counted in `testsCallingDependencyCode` but not in `testsWithUnresolvedTraversal`; `platform-event-subscriber` and `dependency-event-subscriber` reasons.

## Base/Head SHAs

- Claim base (`make-worktree`): `7c35891a63ceb87e20e85e31131d2f79775a9ea2`.
- After the step-11 rebase: **B** = `60434aef127be898c3f95624737aea11404fec08` (`origin/master`), **H** = `264b4106cdef1096c41b4ed9a23870c323969247`.

## Timeline

- 18:03 claim #254 attempt 1 (loop budget charged via `--loop-max 21` on the operator's explicit instruction; 20 already used).
- make-worktree: base `7c35891a`.
- Classified bounded; probes P1–P7 by code reading.
- alsem-main (#160) agreed the scope; asked for a read-only adapter over the snapshot + fixture tests pinning the sets (folded into Design §Code placement).
- Spec round 1 (`spec_rounds` remaining 4; pi_calls 19; subagents 59): sol verdict rework (12 findings), opus approve-with-fixes (20). Every claim source-checked (null drop `format_json.rs:53-70`, snapshot fields `snapshot.rs:887-907`, ledger statuses). Design rewritten for round 2; probes P2/P5/P6/P7/P8 corrected. Register: 32 entries (30 fixed, S7 refuted, O18 fixed with flag-name half refuted).
- Spec round 2 (`spec_rounds` remaining 3; pi_calls 18): diff + all 32 ids, sol via continuation, opus via SendMessage. Both approve-with-fixes: re-raised S1/O1 (parse-incomplete, missing record), S4, S8/O19 (anchor), S9 (--diff case), S12 (timing needs real tests), O12 (TestPage on helpers); new S13 (declaration-only denominator), O21 (schemaVersion), O22 (null-keeping insertion point).
- Spec round 3 (remaining 2; pi_calls 17; subagents 58): 19 ids. opus approve (19/19); sol 18/19, re-raised O12 (TestPage globals).
- Spec round 4 (remaining 1; pi_calls 16): 11 ids (anchor resets). Both approve, 11/11. **Converged**: 35 entries, all accepted by both (sol, opus), no open, blocking entries all fixed (S7 refuted with the issue's own text).
- Step 6 acceptance tests (opus agent; subagents 57): `tests/cli/cli_digest_test_coverage.rs` (9 tests), fixtures `tests/fixtures/digest-test-coverage/{test-selection-graph,tax-interface}.diff`, `tests/fixtures/digest-test-coverage-no-tests/`. `cargo test --test cli cli_digest_test_coverage::` → 0 passed, 9 failed, every one `error: unexpected argument '--tests' found` (exit 2). Fixture deviations: row 3 uses `Ping` (Variant call, `dynamic-target`) because nothing is reached only through `Codeunit.Run(Id)`; row 5 goes through `tax-interface.diff` (a selector cannot name an interface declaration); reasons asserted as a superset; `OnNoSuchEvent` left out. Counts 6/1 in `traversal_incomplete_is_partial` derived from current ledger statuses.
- Step 7 plan: `docs/superpowers/plans/2026-10-23-issue-254-digest-test-coverage.md`, 2 tasks (`plan_tasks` remaining 10). Bounded + 2 tasks → no per-task reviewer.
- T1 (opus implementer; subagents 56, task_attempts T1 → 2): commit `0f74bb08` `digest: test-coverage core (#254)`; 20 unit tests; `check-diff` `{"reasons": []}`; task-affected `3863 tests run: 3863 passed, 8 skipped`. Implementation choices to put before the final panel: (a) `parse-incomplete`/`opaque-dependency` read from the routine's OWN `parse_incomplete`/`body_available` (what `direct_facts_for_routine` reads, `capability_cone.rs:520-536`), not from the coverage record, whose reasons are merged over the whole cone (`capability_cone.rs:2425-2466`) and would blame callers for unreached callees; (b) `no-coverage-record` only for primary routines (a dependency routine already carries `dependency-code`); (c) a ledger status outside the spec table → incomplete reason `unknown-status` (fail-safe); (d) `SubscriberFacts` replaces the plan's `uncovered_reason_facts`. Acceptance test correction for T2: `traversal_incomplete_is_partial` expected `interface-open-world` and 6, but `TaxRate.RatePct` is `polymorphic` with 3 workspace candidates and no unresolved ones → followed per the spec table; correct is 4 tests and reasons `[dynamic-target, object-run-unresolved, test-page, unfetched-dependency, unresolved-receiver-type]`.
- T2 (same implementer via SendMessage; subagents 55, task_attempts T2 → 2): commit `a3239efb`. `--tests` flag, `DigestRequest.tests`, block inserted after `sort_and_drop_nulls` via new `format_json::serialize_normalized_value` (which `serialize_document_value` now calls), human line, schemaVersion history line (still 1.6.0). Every `run_digest_pipeline` caller passes `false` (7 test call sites). `FIXTURE_ROOTS` count pin `36 → 38` in `capability_cone.rs` (two new fixture dirs; the pin exists to catch exactly this). Acceptance test corrected to the exact reason list; new `human_line_with_tests`; matrix row 4 unit+CLI `configured_test_kind_is_not_a_root` with fixture `tests/fixtures/digest-test-coverage-configured-root/`. task-affected `3875 tests run: 3875 passed, 8 skipped`; `cli_b_digest_` 6 passed; `cli_digest_test_coverage::` 11 passed; lib `test_coverage::` 21 passed. Human line wording when complete: `(complete)` (implementer's choice; the spec only shows the partial case).
- Plan doc committed `c021d53a`. Task gate green; `push-branch` → head `c021d53a`; draft PR **#258**.
- Final round 1 (`final_rounds` remaining 4; pi_calls 15; subagents 54): `code-review` skill at high (built-in, ran as a forked skill; 10 candidates, unverified), sol (approve with fixes: 3 important, 1 minor), opus (approve with fixes: 3 important, 6 minor). Deduplicated into FR1–FR18. Fixes by the same implementer (subagents 53, task_attempts F1): `d3860bf6` (FR2–FR6, FR11, FR12, FR14–FR16), `50f0600d` (FR8), `5619401f` (FR17, FR18); FR1 refuted with two CLI tests that passed on the old code; FR7, FR10 refuted by the spec panel's own decisions. Implementer's check-diff ran with `--base <merge-base>` because `master` moved to `60434aef` (#240, #257) and `--base master` lists #240's `.agent/` files; rebase pending (step 11; CHANGELOG conflict expected).
- Final round 2 (`final_rounds` remaining 3; pi_calls 14): diff `c021d53a..5619401f` + FR ids. Timing fallback dispatched (sonnet; subagents 52). sol: 15/16 accepted, FR12 re-raised, new FR19 (raw-attribute handler scan); opus: 16/16 accepted, new FR20 (quoted global TestPage). Fix `8d0994ed` (task_attempts F2). Task gate green (28.2 s).
- Final round 3 (`final_rounds` remaining 2; pi_calls 13): diff `5619401f..8d0994ed`, ids FR6 FR9 FR12 FR13 FR19 FR20. opus: approve, 6/6. sol: FR6 FR9 FR12 FR19 accepted; FR13 re-raised (summed per-test reach still unmeasured — temporary instrumentation asked); FR20 re-raised (plain-spelled global written quoted in the body still missed; opus names the same shape plus keyword names as a minor gap). Dispatched: FR20 root fix to the implementer (task_attempts F3), reach instrumentation in a throwaway source copy (sonnet; subagents 51).
- FR20 root fix `957743cb` (call-site receivers); task gate green (30.6 s).
- Step 11 rebase: old base `7c35891a`, old head `957743cb`, `origin/master` `60434aef` (#240/#257). `git -c core.commentChar=';' rebase origin/master`: clean, no conflict (no `rebase_regate` charge). **Range-diff verdict:** 7 of 8 commits `=`; `a3239efb → 9a24cddf` differs only in `tests/cli/main.rs` context lines (master's new `mod cli_escaped_object_names;`) — no branch change moved, so no extra panel round for the rebase. Register re-pointed: `d3860bf6→9487c59f`, `50f0600d→a6fa03f4`, `5619401f→01130662`, `8d0994ed→7c609c93`, `957743cb→d0aec98c` (anchor hashes kept: range-diff `=`). Evidence texts keep the pre-rebase short SHAs. New head `d0aec98c`. Full gate set started on it.
- Final round 4 (`final_rounds` remaining 1; pi_calls 12): FR13, FR20. opus: approve (both accepted). sol: FR13 accepted; FR20 re-raised (important): `receiver_head` stops an escaped quoted identifier (`"Card ""A"""`) at the first `"`. Fix after the running gates finish (same `target/`); its code change discards those gates' attestability (step 11 re-runs on the new head) and needs round 5, the last the budget covers.
- Full gate set on `d0aec98c` (superseded by the FR20 fix, kept as signal): ci-steps-all 0 (357.7 s, `logs/ci-steps-all/20261023T200050-f84682e3.log`), check-goldens-coverage 0, check-goldens 0 (0.1 s: tree-stamp trust of the green `ci-steps test`), cdo-gate 0 (343.2 s, `logs/cdo-gate/20261023T200649-dfe2f36b.log`); all supervised, queued 0. FR20 escaped-quote fix dispatched (task_attempts F4; subagents 50).
- FR20 fix `264b4106` (`unescape_al_identifier` on both sides); task-affected 3906 passed; task gate green (37.2 s). `origin/master` unchanged (`60434aef`), no rebase.
- Final round 5 (`final_rounds` remaining 0; pi_calls 11): FR20. sol approve, opus approve. **Final panel converged**: 55 entries, all accepted by both. Full gate set started on `264b4106`.

## Discrimination proofs

| test | mutation | fail output | pass output | commit |
|------|----------|-------------|-------------|--------|
| equal_length_event_free_path_wins | `Some(p) > shortest` → `>=` (tie rule flipped) | `left: [("x", "t", true)] right: [("x", "t", false)]` FAILED | ok, 1 passed | 0f74bb08 |
| lowest_id_witness_wins_over_shorter_path | roots iterated `.rev()` | `left: [("x", "t:b", false)] right: [("x", "t:a", false)]` FAILED | ok, 1 passed | 0f74bb08 |
| unresolved_row_flips_status | `g.incomplete.get(n)` → `.filter(\|_\| false)` | `left: "complete" right: "partial"` FAILED | ok, 1 passed | 0f74bb08 |
| ratio_null_when_no_production_changes | ratio → `Some(covered / changed.max(1))` | `left: Some(0.0) right: None` FAILED | ok, 1 passed | 0f74bb08 |
| test_code_excluded_from_denominator | test-code filter dropped | `left: 3 right: 1` FAILED | ok, 1 passed | 0f74bb08 |
| 9 block-reading CLI tests (main_selection_exact_sets, …) | block never inserted (`test_coverage.filter(\|_\| false)`) | 9 FAILED, e.g. `payload.testCoverage missing` (human_line_with_tests, deterministic stay ok, as expected) | 11 passed | a3239efb |
| only_test_code_gives_null_ratio | `sort_and_drop_nulls` re-applied after insertion | `ratio key absent` FAILED (only this test) | 11 passed | a3239efb |
| without_flag_no_block (+ human_line_with_tests) | `tests: tests \|\| true` | `flag off emits testCoverage` FAILED | 11 passed | a3239efb |
| human_line_with_tests | human line suppressed | `left: [] right: ["test coverage: 6 of 10 …"]` FAILED | 11 passed | a3239efb |
| configured_test_kind_is_not_a_root (CLI) | adapter also roots config `test-procedure` kinds | `left: Some(2) right: Some(1)` FAILED | 11 passed | a3239efb |
| configured_test_kind_is_not_a_root (lib) | same | `left: 2 right: 1` FAILED | 21 passed | a3239efb |
| adapter_parse_incomplete_on_primary_and_dependency + adapter_opaque_dependency_is_boundary | FR11: `if dependency { flag dependency-code; continue; }` before own-flag checks | `left: [] right: ["parse-incomplete"]`; `left: ["dependency-code"] right: ["dependency-code", "opaque-dependency"]` | ok | d3860bf6 |
| adapter_parse_incomplete_on_primary_and_dependency | `r.parse_incomplete && false` | `left: [] right: ["parse-incomplete"]` | ok | d3860bf6 |
| adapter_body_unavailable_primary_is_incomplete | primary `!r.body_available && false` | `left: [] right: ["body-unavailable"]` | ok | d3860bf6 |
| adapter_opaque_dependency_is_boundary | dependency `!body_available` arm `&& false` | `left: ["dependency-code"] right: [.., "opaque-dependency"]` | ok | d3860bf6 |
| adapter_dependency_code_and_no_dependency_root | `dependency-code` flag removed | `left: [] right: ["dependency-code"]` | ok | d3860bf6 |
| adapter_no_coverage_record_primary_only | `!recorded.contains(id) && false` | `left: [] right: ["no-coverage-record"]` | ok | d3860bf6 |
| adapter_interface_open_world_shapes | `open && false`; dependency-candidate disjunct `&& false` | `left: [] right: ["interface-open-world"]` (Codeunit 50971; 50926) | ok | d3860bf6 |
| adapter_ambiguous_row_is_incomplete | `"ambiguous"` arm deleted | `left: ["unknown-status"] right: ["ambiguous"]` | ok | d3860bf6 |
| adapter_handler_edges_skip_empty_ids | index from all `ws.routines` | `[("", false), (…50954…, false)]` | ok | d3860bf6 |
| adapter_global_test_page_needs_a_reference | `\|\| true`; reference check `&& false` | IgnoresCard gets `test-page`; UsesCard lacks it | ok | d3860bf6 |
| primary_subscription_is_no_path | `!f.primary_publisher` filter removed | `left: [(sub1, Platform…), (sub2, Dependency…)] right: [NoPath, NoPath]` | ok | d3860bf6 |
| adapter_subscriber_facts_by_publisher | `(None, Some(_)) => {}` | `primary_publisher: false` panic | ok | d3860bf6 |
| not_in_graph_root_counts_uncovered | hit filter `!= DeclarationOnly` | `left: [] right: [("gone", NotInGraph)]` | ok | d3860bf6 |
| coverage_counts_roots_without_an_entry | entries-only filter (old shape) | `left: 0 right: 1` | ok | d3860bf6 |
| dependency_publisher_outside_model_is_a_blind_spot (CLI) | `PublisherRef::Dependency => {}` | `("HandleInWs", …, "no-path")` vs `dependency-event-subscriber` | ok | d3860bf6 |
| dependency_publisher_reached_covers_subscriber (CLI) | dependency app's event-dispatch typed edges dropped | `left: [] right: [("HandleInWs", …, true)]` | ok | d3860bf6 |
| serde_json_map_iterates_in_key_order | built with `--features serde_json/preserve_order` | `left: ["b", "a"] right: ["a", "b"]` | ok (normal build) | 50f0600d |
| sort_keys_keep_nulls_sorts_and_keeps_null | skip `pairs.sort_by` (with preserve_order); drop nulls | unsorted output FAILED; nulls dropped FAILED | ok | 50f0600d |
| adapter_handler_names_ignore_comments | old first-apostrophe scan of `a.raw` restored | edge goes to WrongHandler (`…51023#8d4352ec…`) not RightHandler | ok | 8d0994ed |
| adapter_global_test_page_needs_a_reference (quoted global) | `\|\| !plain(&v.name)` fallback removed | no `test-page` for UsesQuoted (`…Codeunit:51022#d14d043a…`) | ok | 8d0994ed |
| open_interface_still_covers_its_known_candidates | open row's candidate edges dropped after flagging | `left: [] right: [(…50914…, …50944…, false), …]` | ok | 8d0994ed |
| adapter_global_test_page_needs_a_reference | receiver check `&& false` | `UsesQuoted` lacks `test-page` | ok | 957743cb |
| adapter_global_test_page_needs_a_reference | receiver check `&& v.name != "card"` | `UsesCardQuoted` lacks `test-page` | ok | 957743cb |
| adapter_global_test_page_needs_a_reference | receiver check `&& v.name != "action"` | `UsesKeywordName` lacks `test-page` | ok | 957743cb |
| adapter_global_test_page_needs_a_reference | those globals count wherever in scope (old fallback) | `IgnoresQuoted` gets `test-page` | ok | 957743cb |
| adapter_global_test_page_needs_a_reference (escaped quotes) | quoted head ends at the first `"` (old behaviour) | `UsesEscaped` not marked (line 1515) | ok | 264b4106 |
| adapter_global_test_page_needs_a_reference (escaped quotes) | stored name compared without `unescape_al_identifier` | `UsesEscaped` not marked (line 1515) | ok | 264b4106 |
| adapter_on_test_selection_graph | `"HandlerFunctions"` → `"HandlerFunctionsX"` | covered lacks `("InModalHandler", "OpenCardModally", false)` FAILED (first attempt passed — OpenCardModally was already incomplete via `object-run-unresolved`; `InModalHandler` added to the selection, then it failed) | 20 passed | 0f74bb08 |

## Acceptance result

Every row of the acceptance matrix is met, proved by `tests/cli/cli_digest_test_coverage.rs` and the `test_coverage::` lib tests: 1 `main_selection_exact_sets` (direct call, `viaEvent` false); 2 `main_selection_exact_sets` (Invoice.al subscribers, `viaEvent` true) + `subscriber_via_publisher_is_covered` + `dependency_publisher_reached_covers_subscriber`; 3 `main_selection_exact_sets` (`Ping` uncovered `no-path`) + `traversal_incomplete_is_partial` (exact reasons, 4 / 1) + `unresolved_row_flips_status`; 4 `test_code_is_excluded` + `configured_test_kind_is_not_a_root` (CLI and lib); 5 the `block()` partition check on every CLI run + `partition_is_exact_disjoint_sorted` + `interface_declaration_is_declaration_only`; 6 `deterministic_runs_are_byte_identical` (two processes); 7 `main_selection_exact_sets`, `diff_input_exact_sets`, `no_tests_workspace`, `only_test_code_gives_null_ratio`. Final panel: both reviewers marked items 1–7 met.

## Findings register summary

55 entries: spec panel S1–S13, O1–O22 (4 rounds); final panel FR1–FR20 (5 rounds; the built-in `code-review` at high ran as the specified step, as a forked skill). 50 `fixed`, 5 `refuted` (S7 flag vs always-on; FR1 dependency-publisher key, disproved by two CLI tests that passed on the old code; FR7 second graph; FR9 per-test search cost, disproved by measurement; FR10 TestRunner codeunits). Every entry accepted by both reviewers (`sol` = gpt-6.1-sol in the astra slot, `opus` = Opus in the flash slot); no `open`, no blocking entry `deferred`.

## Gate results

| gate | exit | log | notes |
|------|------|-----|-------|
| ci-steps-task (after T2) | 0 | `.agent/runs/20261023-180348-ce43e4/logs/ci-steps-task/20261023T190352-41da03d1.log` | 31.4 s, supervised |
| ci-steps-task (after final r1 fixes, head `5619401f`) | 0 | `.agent/runs/20261023-180348-ce43e4/logs/ci-steps-task/20261023T193341-8cb106f2.log` | 29.4 s, supervised |
| bench (timing, CDO `Test` app) | 0 | `.agent/runs/20261023-180348-ce43e4/logs/bench/` (3rd run; runs 1–2 failed on missing `/usr/bin/time` / `bc` and an absolute `--changed` path) | release-fast `alsem`, `--changed src/TestFramework/CDOLibraryEmailTemplate.Codeunit.al`: T = 1844 `[Test]` procedures, 34 changed, 34 covered; flag off 0.753 / 0.819 s, on 0.819 / 0.786 s (no measurable cost); the two `on` outputs byte-identical. Reach is small: the `Test` app has no package cache, so its calls into `Cloud` stop at the dependency boundary (`testsCallingDependencyCode` 1837). Summed per-test reach is not exposed by the CLI. Pre-stated fallback (Cloud copy + 200 generated tests) run next. |
| bench (scale fallback: Cloud copy + generated test codeunit) | 0 | `.agent/runs/20261023-180348-ce43e4/logs/bench/20261023T194504-a5d79e60.log` (153.6 s) | sonnet agent. Copy `/work/bench/254/cloud-copy` (pinned baseline untouched) + `ZZ Bench 254 Tests` (id 6799999) with **108** `[Test]` procedures (only 108 public zero-parameter non-event procedures exist; 35 codeunits) — fewer than the planned 200. `--file src/Features/Email/Templates/CDOEMailTemplateManagement.Codeunit.al` (1913 lines; `--changed` reads an existing file as a diff, so the first attempt selected nothing). Binary release-fast at `5619401f` (`8d0994ed` changes no search code). Walls off/on: 35.08/29.48 (cold), 22.08/22.87, 22.01/22.11 s → medians 22.08 vs 22.87 s; warm on−off +0.78 and +0.10 s (within noise; ~22 s is the digest itself; output 21.7 MB). T = 108, changed 63, covered 38, ratio 0.603, partial (ambiguous, dependency-code, dynamic-target, object-run-unresolved, unfetched-dependency), testsCallingDependencyCode 38, testsWithUnresolvedTraversal 4. 3 `on` outputs byte-identical, 3 `off` byte-identical. Not measured here: BFS time alone, per-test reach size (next row). |
| reach instrumentation (throwaway copy, not a gate) | — | `/work/bench/254/src-instr` (archive of `8d0994ed` + one `eprintln!` behind `ALSEM_254_REACH`), target `/work/bench/254/target-instr`; sonnet agent | Cloud copy, 3 identical runs: `roots=108 summed_reach=2800 max_reach=865 bfs_ms=1`. CDO `Test` app: `roots=1844 summed_reach=9998 max_reach=29 bfs_ms=3`. `bfs_ms` covers the whole root loop (search + witness/reason bookkeeping). testProceduresConsidered = 108 = roots. The search is ~1–3 ms of a ~0.8–22 s digest. Nothing in the worktree or main checkout changed. |

**Attested gate set on H = `264b4106`** (all supervised, queued 0 s):

| gate | exit | seconds | log |
|------|------|---------|-----|
| ci-steps-all | 0 | 374.5 | `.agent/runs/20261023-180348-ce43e4/logs/ci-steps-all/20261023T201857-50a90d00.log` |
| check-goldens-coverage | 0 | 0.0 | `.agent/runs/20261023-180348-ce43e4/logs/check-goldens-coverage/20261023T202512-757e5ae4.log` |
| check-goldens | 0 | 0.1 (tree-stamp trust of the green `ci-steps test` on this exact tree) | `.agent/runs/20261023-180348-ce43e4/logs/check-goldens/20261023T202512-4ee1fabc.log` |
| cdo-gate | 0 | 304.5 | `.agent/runs/20261023-180348-ce43e4/logs/cdo-gate/20261023T202513-21d54ce1.log` |

`git status --porcelain -- . ':!.agent'` empty after the gates. CDO workspace: the pinned `cdo_ws` of `.agent/config.json`; cdo-gate exited 0, so its ratchets (real-unknown rate, unknown count, `ambiguousResolved` pin, coverage contract) all held; the change adds an opt-in output and does not touch resolution.

CI on draft PR #258 at `c021d53a`: test pass (3m29s), release pass, GitGuardian pass.

## Discoveries

```json
[
  {
    "subsystem": "l4 / combined graph (record operations → platform events)",
    "locator": "src/program/model/events.rs:271-276, 499-503 (platform events have publisher_routine_id None); src/engine/l4/combined_graph.rs:874-882 (typed event-dispatch needs a model publisher routine); surfaced by src/engine/l5/test_coverage.rs (uncovered reason platform-event-subscriber)",
    "symptom": "A subscriber to a table's platform event (OnAfterInsertEvent, OnAfterModifyEvent, OnAfterValidateEvent, ...) has no incoming edge in the digest/snapshot graph, so a routine that does Rec.Insert(true) never reaches it. `digest --tests` therefore reports every such subscriber `uncovered` with reason `platform-event-subscriber`, even when a test inserts the record; digest effect paths cannot cross that dispatch either.",
    "kind": "enhancement",
    "origin_issue": 254,
    "reproducer": "tests/fixtures/test-selection-graph: `alsem digest <ws> --routine OnAfterInsertLedger --tests --format json` gives uncovered `platform-event-subscriber`; add a [Test] that does `Ledger.Insert(true)` on that table and it stays uncovered.",
    "pre_existing": true,
    "capability": "a record operation that runs triggers (Insert/Modify/Delete/Validate with RunTrigger true, as the implicit-trigger edges already decide) dispatches to the subscribers of the matching platform events, as an edge the graph's consumers can follow",
    "acceptance": "For a table with an OnAfterInsertEvent subscriber, a routine doing Rec.Insert(true) has an edge to that subscriber (kind event-dispatch or a new platform-event kind) and Rec.Insert(false) has none; `digest --tests` covers the subscriber from a test that inserts with true; the CDO unknown rate and existing goldens move only where such subscribers exist, each triaged."
  },
  {
    "subsystem": "l5 / digest --tests (TestPage-driven code)",
    "locator": "src/engine/l5/test_coverage.rs drives_test_page (reason test-page); no edge exists from a TestPage method call (OpenView, Trap, Action.Invoke, field SetValue) to the page's triggers/actions",
    "symptom": "A test that drives a page through a TestPage variable covers none of the page's code: the page's OnOpenPage/OnAfterGetRecord, field OnValidate triggers and action OnAction triggers are not reached, so the routines they call are uncovered and the block is `partial` with `test-page`. BC test codeunits use TestPage constantly, so for UI-heavy solutions the ratio understates real coverage.",
    "kind": "enhancement",
    "origin_issue": 254,
    "reproducer": "tests/fixtures/test-selection-graph: `alsem digest <ws> --routine <a page action or field trigger routine> --tests` — the test using `TestPage` (CardShowsRecord) does not reach it; the block lists `test-page`.",
    "pre_existing": true,
    "capability": "a TestPage variable's method calls resolve to the page object's lifecycle triggers and the named action/field triggers, as resolved (or candidate-set) edges `digest --tests` can follow",
    "acceptance": "On a fixture with a page action and a field OnValidate, a [Test] doing `P.OpenView(); P.\"My Action\".Invoke(); P.Field.SetValue(x)` covers the action's OnAction, the field's OnValidate and OnOpenPage; the `test-page` reason is reported only for TestPage calls that do not resolve; a test with no TestPage is unaffected."
  }
]
```

## Not delivered

- The scale timing used 108 generated `[Test]` procedures, not the 200 the spec named: the `Cloud` copy has only 108 public zero-parameter non-event procedures. T = 1844 was also measured on the CDO `Test` app (small reach). Both showed no measurable cost (search 1–3 ms).
- Stated limits, all in the CHANGELOG and the module doc: subscribers to events raised only inside dependency code or to platform events are uncovered with their reason (first discovery above); pages driven through `TestPage` are not followed (`test-page`, second discovery); a test codeunit's `OnRun` is not a root; `Subtype = TestRunner` and test-library codeunits count as production; a quoted TestPage global used only as a call argument is not detected on the caller (the callee that owns the parameter is).
