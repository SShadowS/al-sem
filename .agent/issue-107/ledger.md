# Issue #107 ledger

## Claim

```json
{"run_id":"20261019-071107-16946d","issue":107,"attempt":1,"session":"claude-code-session:c26c6ca6-002c-49f4-bd91-33f31cbdf028","branch":"issue/107-agent-discovery-l4-summary-solve-a1","worktree":"U:\\Git\\al-sem-issue-107-a1","body_hash":"f6c37d50d40dae37d3ac3ac32e0e9fcf26a32cd24ea7f73589fd01c80246245a"}
```

Issue body hash (sha256): `f6c37d50d40dae37d3ac3ac32e0e9fcf26a32cd24ea7f73589fd01c80246245a`.
Worktree base (`make-worktree`): `8405f96dd6ef849a37ee3cce327185174abec537`; build output at `H:\al-sem-targets\al-sem-issue-107-a1` (`target_root`).

Reviewer roster (operator instruction for this loop, 2026-10-18: "Dont use gemini. USe gpt 6.1 sol and local opus 5.5"): `sol` = gpt-6.1-sol via pi (astra slot), `opus` = a local Opus 5.5 subagent (flash slot). Attest with `--substitute astra=sol --substitute flash=opus`.

## Classification

**architectural.** An event publisher's parameter roles (`RecordRoleSummary`: `dirty_at_exit`, entry requirements, loaded fields) come only from its own body walk and its source callsites (`summary_runner.rs:635-645` c1b over `routine.call_sites`, `:738-774` the CFG walk). An integration/business event publisher has an empty body, and its event-dispatch edges to subscribers carry no callsite (`combined_graph.rs:332-340`), so the subscribers' effects on `var` parameters never reach the publisher's roles, and so never reach a caller that raises the event. The fix crosses the L4 solver's fixpoint ordering, the event-dispatch model (AL subscribers bind parameters by NAME and may declare a subset; `Sender`/element parameters), and workspace vs dependency (ABI) publishers, and it can move L4-backed detector output.

## Assumption probes

14 rows, in the spec (`docs/superpowers/specs/2026-10-19-issue-107-publisher-roles-subscribers-design.md`, "Assumption probes"); fixtures and outputs under `.agent/runs/20261019-071107-16946d/probes/` in the main checkout. Falsified: (a') the acceptance's "dirty at exit" for a `Modify` subscriber (in this engine `Modify` makes the record `Persisted`, not dirty; the oracle becomes "equal to a direct call to the subscriber"); (b2) a symbol-only dependency publisher shows no caller-visible defect (callers already see it as opaque). Confirmed: the workspace reproducer; the source-bearing dependency publisher defect in its body-local facts; subscribers settle before the publisher (dispatch edges are in the SCC input); binding is by name, with a second inline copy of the mapping in `pd_solve::carry_reqs` that does not trim quotes; all 190 CDO publishers have empty bodies.

## Base/Head SHAs

- B = `6c6bce1768d873a5bcc99d82bb4b4b2fdc55dadf` (`origin/master`, #172), H = `674743ef547f4f83982d8967c84e54171461d548`. `check-diff --base B --head H`: `{"reasons": []}`. Final panel converged at round 5 (`final-r5-{sol,opus}.md`): both reviewers accept all 41 entries; no new findings. Caps: final_rounds 5/5, rebase_regate 1/1, pi_calls 10 left, subagents 25 left.

## Timeline

- claim + make-worktree: `{base: 8405f96d, target: H:\al-sem-targets\al-sem-issue-107-a1}`.
- classify: architectural.
- probes + spec draft (opus subagent): 14 probe rows; spec written (Goal, probes, Non-goals, Acceptance matrix, Design 1-7, False-positive and failure shapes, Measurement plan).
- baseline CDO (base `8405f96d`, release-fast, pinned workspace `bc3ccb18`, 0 dirty paths), `cdo-measure`: fingerprint 6.6 s, analyze primary 39.1 s, analyze all 48.2 s, all exit 0 (session scratchpad `107/before`, `107/measure/before`).
- spec round 1 (`spec-r1-{sol,opus}.md`, run dir): sol 8 important + 2 minor; opus 3 important + 9 minor. Merged into R1-1..R1-16. Spec revised: MUST load facts masked for subscribers that may not run (`conditional` edge flag), exact enumeration (orders x optional subsets, n <= 4) replacing the closure, no monotonicity claim (capped SCC -> opaque role), symbol-only publishers unchanged, quote trimming unified with its outputs named, resolved/deduped bindings, isolated = optional, d3 dropped, d40 explicit, counters, 13 unit rows.
- spec round 2: opus 15/16 (R1-9 isolated contradiction), sol 14/16 (R1-3, R1-9) + 3 important. R2-1..R2-7: presence visibility vs path opacity; `optional_load` (yes -> no: the only value that neither adds nor destroys a proven load); `conditional_any` (union over readings); unseen dispatch = the existing opaque callee.
- spec round 3: opus 23/23 (2 minor wording); sol 21/23 (R1-13, R2-3) + 1 important (R3-1, a masked `Copy` hiding a mutation). One rule: the mask touches only the published load trio, every MAY fact from unmasked effects.
- spec round 4: opus 24/24, 4 notes; sol 23/24, re-raised R3-1 (per-binding recompute of `mutates_param` loses an earlier call's `yes`). Fixed by the conductor in the spec: c1b accumulates `mutates_param` (join with the prior value), rows 5d'/5d''.
- spec round 5: BOTH 24/24 accepted, no blocking findings. **Converged** (`_register_failures`: none). Caps: spec_rounds 5/5, pi_calls 5, subagents 9.

- step 6 acceptance tests (opus; report `.agent/runs/20261019-071107-16946d/acceptance-tests.md`), not committed: `tests/gap/gap_issue107_publisher_roles.rs` (22 tests; conductor-verified `cargo nextest run --test gap -E 'test(/gap_issue107/)'` → 22 failed, e.g. `OnRecModify.persistsCurrentRecord: want Yes, got No`); `capability_cone.rs` `issue107_dependency_publisher_presence_composes_subscribers` (red) and `issue107_symbol_only_dependency_publisher_roles_unchanged` (green by design: a no-regression pin); `pd_solve.rs` row-13 `carry_reqs` test (red: quoted `"Rec"` does not bind); compile-red until implemented: `summary_runner.rs` `issue107_pending_tests` (`event_publisher_param`, `EventEdge.conditional`, `CombinedEdge.dispatch`, `with_roles_iteration_cap`) and `index.rs` (`Subscription.conditional_any`).
- Ruling (step 6): two overloads of one event share one event id, so the engine gives each overload a `resolved` dispatch edge to the subscribers of either overload (found by the test writer). A `resolved` edge to a subscriber whose declared parameters (other than `Sender`; element parameters exist only on platform publishers, which build no dispatch edge, `combined_graph.rs:362`, so the code exempts only `sender`) do not ALL bind by name to this publisher overload is treated like a non-resolved edge: optional and path-opaque, never exact. Reason: AL binds a subscriber to one overload; the engine cannot tell which, so an exact composition could add another overload's subscriber's effects. Cost if wrong: an over-cautious (more `unknown`) role for overloaded events, which only suppresses findings. Row 7 uses a parameterless subscriber for the `ambiguous` edge; a row for the overload-collision case is added in the plan.
- Ruling (step 6, revised at final-panel round 1): overloads of one event share one event id, so the engine gives every overload a `resolved` dispatch edge to the subscribers of either overload, and a smaller subscriber's parameter names all bind the larger overload too (sol's final-panel `OnPair(Rec)` / `OnPair(Rec, Qty)` / `Handle(Rec)` case). Name compatibility does not establish which overload AL selected, and the engine does not keep it. So: (a) every binding of a publisher whose event id more than one publisher routine raises (`event_dispatch::shared_event_ids`, over the combined graph's `event-dispatch` edges) is optional AND path-opaque, never exact; (b) as before, a `resolved` edge to a subscriber with a declared parameter (other than `Sender`) that binds no parameter of this overload is `Opaque` (the dispatch is unseen). Reason: an exact composition could add another overload's subscriber's effects or a proven load it never performs. Cost if wrong: MAY facts stay, the load trio goes `yes -> no` and path facts `unknown` on overloaded events; this only suppresses findings. Rows 7d (larger subscriber) and 7e (smaller subscriber) pin it. (Supersedes the step-6 overload Ruling above.)
- Ruling (step 6, overloads; revised again after final-panel round 2, SUPERSEDES both above): overloads of one event share one event id (`encode_event_id` has no signature), so a subscriber AL binds to ONE overload gets a dispatch edge from EVERY overload, and the engine does not keep which overload a subscription selected. A publisher with any `event-dispatch` edge whose event id more than one publisher routine raises (`shared_event_ids`) therefore dispatches as UNSEEN for every record parameter: the opaque-subscriber / unknown-callee contract, every presence and path fact `unknown`. The round-1 optional + path-opaque bindings were rejected: they mask only the load trio, so a MAY fact of a subscriber that selected the other overload (a `Reset`) still reached callers as `yes` and produced a false d41 finding (F2-1, pinned by row 7f through the detector). This over-approximation only loses precision and never adds a `yes`; it also covers one publisher duplicated in two source roots (harmless). Census: these parameters count under `unseen_opaque`; `isolated_path_opaque` counts isolated bindings only. Cost if wrong: less precise roles on overloaded events, suppressing only.
- Ruling (step 6): the forced roles-cap test needs a test-only cap hook; named `with_roles_iteration_cap` (`#[cfg(test)]`). Cost if wrong: a rename.

- step 7 plan `docs/superpowers/plans/2026-10-19-issue-107-publisher-roles-subscribers.md` (7 tasks; `plan_tasks` charged 7), committed `6aebed9e`; spec committed `eaca18f3`.
- Task 1 (opus): parked the acceptance tests (copies in `.agent/issue-107/pending-tests/`, named stash `stash@{0}`, never popped); c1b accumulates `mutates_param`; commit `be6d73b0`; `check-goldens` (no regen) 872/872, zero golden files moved; `ci-steps task` 3440/3440. No discrimination proof by design (nothing observes it yet); its pin is row 5d', proved in Task 4.
- Ruling: `check-diff` crashed with `UnicodeDecodeError: 'charmap' codec can't decode byte 0x90` (the executor decodes git output as cp1252; the committed spec/plan hold UTF-8 symbols): this is open issue #95, live. The flow may not edit `scripts/`, so executor calls that read git text run with `PYTHONUTF8=1` (Python's UTF-8 mode, no code change); the check then runs normally. Cost if wrong: none (decoding only).
- Ruling: `check-diff --base master` compared trees against a `master` that has moved past this branch's base (#160 step 1 `576dd296` landed, with its `.agent/issue-160/` evidence), so it reported `protected-path:.agent/issue-160/*` that this branch never touched. The per-task check uses the fork point `--base 8405f96d` (= `git merge-base HEAD origin/master`): `{"reasons": []}`. Step 12 uses the attested `B` after the rebase. Cost if wrong: none.

- Task 1 review (sonnet): approve, 3 minors (minor 3, prove the accumulation pin by dropping each join term, carried into Task 4).
- Task 2 (opus): one subscriber->publisher mapping `event_publisher_param` (quotes ignored) for `event_param_temp_state` and `pd_solve::carry_reqs`; commit `b1842276`; no golden moved; both discrimination breaks failed as expected (report `task2-report.md`). Review (sonnet): approve, 5 minors; minor 1, a third by-name copy (reverse direction) in `program/model/event_param_temp.rs:127`, fixed by the review-fix (sonnet): one `event_param_names_match` in `program::model` used by both directions, commit `39870117`, lib tests / check-goldens (no regen) / `ci-steps task` exit 0 (`task2-fix.md`); minors 2-5 not applied (reasons in `task2-fix.md`). `check-diff --base 8405f96d`: no reasons.

- Task 3 (opus): event edges carry `conditional` (from `Subscription.conditional_any`, the union over readings; intersected `conditions` and its readers unchanged) and isolated; commit `ca781364`; no golden moved; 3 discrimination breaks failed/passed (`task3-report.md`). Review (sonnet): approve, 3 minors.
- Task 4 (opus): dispatch bindings + presence fold with `optional_load`, MAY facts from unmasked effects, `mutates_param` shadow/accumulation, `fixed_roles` presence-only, symbol-only unchanged; commit `f8c3a359` (`task4-report.md`). Goldens: 4 r3a2 files, 18 lines, each a publisher / caller role named by spec Design 7 (incl. the unseen dispatch in `ws-event-fanout`); no other family moved; receipt `.agent/golden-triage.md`; `ci-steps task`, `check-goldens`, pre-commit green.
- Task 4 Ruling: `ExactBinding.path_opaque` and `Unseen { too_many }` land with their first readers (Tasks 5/6): unread now, they fail `-D warnings` dead-code, and `#[allow(` is forbidden. Cost if wrong: none.
- Task 4 Ruling: proof (v) for rows 5d'/5d'': reverting to the full per-binding overwrite fails BOTH orders (the publisher's own dispatch fold goes through the same shared function); the predicted split (5d' fails, 5d'' passes) appears when only the `p.mutates_param` term is dropped; dropping the callee's `mutates_param` term fails both. All three recorded. Cost if wrong: none (each term has a failing break).
- Task 4 Ruling: golden triage by hand (18 lines) instead of the triager agent; `tests/r3a2-goldens/manifest.json` (hand-maintained matrix, not written by the regen) updated with Edit, total 44 → 52 and the four per-fixture counts; a `RoleRow` type alias added to the parked capability_cone test helper because clippy `type_complexity` failed the task gate. Cost if wrong: a stale manifest number, caught by the r3a2 test.

- Task 4 review (sonnet): approve, 2 important (test gaps). Review fix (sonnet, `task4-fix.md`), commit `d4f1d8b5`, no code change, no bug found: new gap tests `review_isolated_loader_is_masked` (break: drop `|| d.isolated`), `review_duplicate_edges_or_their_flags` (`|=` -> no-op), `review_four_subscribers_stay_exact_even_with_a_duplicate_edge` (dedupe find never matches; and `>` -> `>=` at the n=4 boundary), `review_unknown_subscriber_routine_is_unseen` (missing subscriber -> false): each failed under its break, passed after revert; non-resolved edge covered by Task 5 rows 7/7b and proof (v); `ci-steps task` exit 0. Shared helpers `SIMPLE` / `simple_edge` now live in the gap file (Task 5 must not re-place the parked copies).

- Task 5 (opus): the walker applies the dispatch by exact enumeration (n <= 4), opaque fallback for unseen dispatch, path-opaque isolated bindings; commit `ebb3db6c`; `ci-steps task` 3478 passed (`task5-report.md`). Goldens: 11 files (7 r3a2 summary-core, 4 cli-a stats/json of `ws-d8-commit-in-tx`), each a Design 7 output; the cli-a moves are d40/d42 skip-reason counts only, no finding added or removed; the r3a2 manifest did not move. Proofs: break (v) (non-resolved rule) fails rows 7/7b; rows 4/4b/12b were green after Task 4 and each fails under its own break; two stated limits: break (ii) (always add the empty sequence) leaves row 4b green (the unit test catches it), break (vi) (drop the dedupe) leaves row 7c green (the two Task 4 review tests catch it).
- Task 5 Ruling: for unseen dispatch or an isolated binding, the walker also sets the publisher's required entry fields to `Unknown` while the record is not loaded (`opaque_transfer` alone left `Known([])`): this is what the spec's "every path fact unknown" and Design 5's opaque role require, and rows 6, 7, 7b, 7d, 12 assert it; `opaque_transfer` itself is unchanged, so ordinary call sites do not move. Cost if wrong: a less precise field list for unseen dispatch, suppressing only.
- Task 5 process note (recorded, not hidden): the first write of the regen receipt failed, and the `--regen` run passed the gate on Task 4's still-fresh receipt; the Task 5 receipt was written after the regen, from the triage done before it; the regenerated files are exactly the 11 triaged lines.
- Task 5: `walk_param` is `pub(crate)` (its new parameter type is crate-private); `Unseen { too_many }` lands in Task 6 with its reader.
- Discovery candidate 3 (Task 5): in `ws-event-cycle` a dispatch that closes a recursive SCC leaves all 5 members' entry facts `unknown` (each starts from its base role, whose entry facts are `unknown`, and the join never lowers it); sound but imprecise (the true value is `no`). Capability: the roles Jacobi starts recursive members from bottom for the entry facts.

- Task 5 review (sonnet): approve, 3 minors.
- Task 6 (opus): a capped roles SCC publishes the opaque role; dispatch census counters; `with_roles_iteration_cap` (`#[cfg(test)]`); commit `63b17f43`; check-goldens 872/872, zero moves; `ci-steps task` 3479 passed (`task6-report.md`). Review (sonnet): 2 important: the cap test `iterations >= cap` (pre-existing, now observable because a cap publishes the opaque role) also fired for an SCC that converged on exactly the cap round; no negative control. Review fix (sonnet, `task6-fix.md`): `changed && iterations >= cap`; new `converged_roles_scc_is_never_treated_as_capped` (same SCC converges on round 3: cap 2 caps, cap 3 / 1000 / none keep real roles, no cap recorded); break (guard removed) fails "cap 3: converged SCC records no cap", restored passes; check-goldens and `ci-steps task` exit 0.
- All 112 parked test functions are placed in the committed tree (checked by name).

## Plan notes (spec rounds 4-5, non-blocking)

- The publisher's own `mutates_param`: compute it from the unmasked copies shadow and the accumulation in ONE final step after the call-site loop and the dispatch fold, so no per-binding recompute can overwrite it (both). Discrimination: revert to the overwrite -> 5d' fails, 5d'' passes.
- Land the c1b `cr.mutates_param` join (the R3-1 "no-op today" claim) as its own commit with a zero-golden-move check-goldens run, before the dispatch step is wired in (opus).
- Row 12: add the by-value publisher-parameter case (no presence change; caller keeps loaded state and narrowing) (opus).
- Force the roles-cap branch in a unit test: 0 cap hits on fixtures/CDO does not exercise the opaque-role replacement (sol).
- Regressions: `OnManualCopy; OpaqueHelper`; a subscriber that forwards another event; two forwarding levels (sol).
- "Exactly the runs AL can make" means the abstract optional-binding model; shared binding/license conditions can correlate subscribers (sol, wording).
- The two discoveries (d41 / load-point bodyless guard; the order-blind `validates && Unknown` rule) are filed after the merge; the plan names them as the owners of the stated leniencies (opus).

## Acceptance matrix

## Discrimination proofs

## Findings register summary

## Gate results

Step 9 on `238d5ba9` (each through `agentflow run`, `supervised: true`, `CDO_WS` per call); tree clean afterwards. Logs under `.agent/runs/20261019-071107-16946d/logs/` (main checkout).

| Gate | Exit | Seconds | Queued | Log |
|---|---|---|---|---|
| `ci-steps-all` | 0 | 685.8 | 0 | `ci-steps-all/20261019T104354-84ef190f.log` |
| `check-goldens-coverage` | 0 | 0.3 | 0 | `check-goldens-coverage/20261019T105521-80cc8f6f.log` |
| `check-goldens` | 0 | 36.1 | 0 | `check-goldens/20261019T105522-a74e6296.log` |
| `cdo-gate` | 0 | 251.1 | 0 | `cdo-gate/20261019T105559-181a12c1.log` |

**Final head `0f6300ba`** (after the code-review fixes `603b514a`, the spec wording `e1d8fc23` and the final-panel round-1 fixes `0f6300ba`), same supervisor, `supervised: true`, tree clean afterwards:

| Gate | Exit | Seconds | Log |
|---|---|---|---|
| `ci-steps-all` | 0 | 589.1 | `ci-steps-all/20261019T120512-b9bf5a1b.log` |
| `check-goldens-coverage` | 0 | 0.3 | `check-goldens-coverage/20261019T121501-89a923af.log` |
| `check-goldens` | 0 | 32.0 | `check-goldens/20261019T121502-71f3b672.log` |
| `cdo-gate` | 0 | 168.3 | `cdo-gate/20261019T121535-26467d5e.log` |

(A gate run started on `603b514a` was stopped by the conductor: a docs commit `e1d8fc23` landed in the worktree mid-run, so that run could not attest the final head; its orphaned `perf_bounds` child was killed after the gate semaphore confirmed it was this run's.)
CDO re-measure at `0f6300ba` (same pinned workspace, 0 dirty): analyze primary 2277 -> 2277, all 30712 -> 30712, 0 fingerprints added/removed; explicit d37/d39/d40/d41/d42 primary 160 -> 160, all 493 -> 493, 0 added / removed / changed; call-graph stats byte-identical. Census at the final head (the split from F1-CR7): publishers 459 (0 with a body); bindings per parameter 1:601, 2:60, 3:10, 4:10, >4:21, max 15; `exact_walker` 0, `exact_fixed_roles` 680, unseen-opaque 0, too-many 21, isolated/overload path-opaque 1; walker sequences 0; recursive dispatch rounds 0; mixed readings 0. This confirms the investigation below: on CDO only the presence fold of dependency publishers runs.

**Final head `0712660d`** (after the final-panel round-2 fix: overload siblings dispatch as unseen):

| Gate | Exit | Seconds | Log |
|---|---|---|---|
| `ci-steps-all` | 0 | 612.0 | `ci-steps-all/20261019T123558-3acb2a82.log` |
| `check-goldens-coverage` | 0 | 0.6 | `check-goldens-coverage/20261019T124611-eecd7634.log` |
| `check-goldens` | 0 | 35.7 | `check-goldens/20261019T124612-c77335bd.log` |
| `cdo-gate` | 0 | 167.4 | `cdo-gate/20261019T124649-7655d0fd.log` |

Tree clean afterwards. CDO at `0712660d`: analyze primary 2277 -> 2277, all 30712 -> 30712 (0 fingerprints added/removed); d37/d39/d40/d41/d42 primary 160 -> 160, all 493 -> 493, 0 added / removed / changed; call-graph stats identical. Census unchanged from `0f6300ba` (`unseen_opaque` 0: CDO has no overload-sibling publisher; `isolated_path_opaque` 1).

**Step 11, rebase onto `origin/master` (#160 step 1 `576dd296`)**: `rebase_regate` charged (1/1). One textual conflict, `CHANGELOG.md` (both added Fixed entries; both kept). `git range-diff 8405f96d..0712660d origin/master..HEAD`: only the CHANGELOG commit differs. (Process note: the commit message `#107: CHANGELOG` starts with `#`, which git's default comment char strips; committed with `--cleanup=verbatim`, rest of the rebase with `core.commentChar=';'`.) A first gate run on the rebased head `235d7058` was killed by Claude Code's memory reaper while idle (the operator restarted it); the restarted run went RED: `ci-steps-all` and `cdo-gate` exit 1, both `E0308` in the two new cross-app role tests (`capability_cone.rs:5251`, `:5311`): #160 step 1 changed `build_cross_app_base` to take `&Arc<InputSet>`. A semantic conflict the textual rebase did not show. Fixed in `8336e494` the way #160 adapted its own callers (`InputSet::read(&ws, &Default::default())` first); `ci-steps task` 3519 passed. Gates on `8336e494`:

| Gate | Exit | Seconds | Log |
|---|---|---|---|
| `ci-steps-all` | 0 | 644.7 | `ci-steps-all/20261019T140008-47f53c20.log` |
| `check-goldens-coverage` | 0 | 0.4 | `check-goldens-coverage/20261019T141054-86ed0ec3.log` |
| `check-goldens` | 0 | 36.2 | `check-goldens/20261019T141055-c40c9ea5.log` |
| `cdo-gate` | 0 | 160.8 | `cdo-gate/20261019T141132-90b06672.log` |

Tree clean afterwards. No CDO before/after re-run: the new base carries #160's own changes, so a before/after against `8405f96d` would not isolate #107; `cdo-gate`'s ratchets cover the rebased head.

**Final panel round 4** (`final_rounds` charged, 1 left; briefing `final-r4-briefing.md`): sol (`final-r4-sol.md`) and opus (`final-r4-opus.md`) both accepted all 41 entries, F4-1 included; no new findings.

**Second rebase.** Before attest, `check-diff --base origin/master` flagged `.agent/issue-160/*`, because `origin/master` had moved to `6c6bce17` (#172, #160 step 1b). That commit touches no `src/engine/l4/` file; only CHANGELOG overlaps. `git merge-tree` showed no conflict, and the rebase `8336e494` → `674743ef` (17 commits) applied clean, so `rebase_regate` was not charged. Step 9 gates on `674743ef` (all `supervised: true`; gate-run records show head `674743ef…` and a clean tree after each):

| Gate | Exit | Log |
|---|---|---|
| `ci-steps-all` | 0 | `ci-steps-all/20261019T142011-ab1a0504.log` |
| `check-goldens-coverage` | 0 | `check-goldens-coverage/20261019T143204-166d5824.log` |
| `check-goldens` | 0 | `check-goldens/20261019T143205-cba81038.log` |
| `cdo-gate` | 0 | `cdo-gate/20261019T143244-0f203aa2.log` |

`git status --porcelain -- . ':!.agent'` empty afterwards.

### CDO measurement

Pinned CDO baseline (`CDO_WS`) at `bc3ccb18`, 0 dirty paths, both sides; release-fast before = base `8405f96d`, after = `238d5ba9`.
- `cdo-measure`: analyze primary 2277 -> 2277, all 30712 -> 30712; 0 fingerprints added/removed at either scope. Wall (indicative): fingerprint 6.6 -> 8.8 s, primary 39.1 -> 38.5 s, all 48.2 -> 39.7 s.
- Explicit role readers (`--detector d37,d39,d40,d41,d42`, full ids), both scopes: primary d37 12 / d39 3 / d40 143 / d42 2, all d37 345 / d39 3 / d40 143 / d42 2, identical before and after (0 added, 0 removed, 0 changed payloads). d41 none either side.
- `aldump --program-call-graph-stats`: byte-identical.
- Census (`ALSEM_SUMMARIES_CENSUS=1`, after): publishers 459 (0 with a body); bindings per parameter 1:601, 2:60, 3:10, 4:10, >4:21, max 15; exact 680, unseen-opaque 0, too-many 21, isolated path-opaque 1; walker sequences 0; recursive dispatch rounds 0; mixed readings 0.
- Investigation (opus, `cdo-census-investigation.md`, probes in `probes2/`): NO defect. The walker step is wired into `alsem analyze` (`apply_dispatch` <- `walk_param` <- `compose_roles_only` <- `member_roles` <- `run_one_scc_roles` <- `compute_summaries_v2_bundle_with_leaves`, the same code the r3a2/r3a5 goldens run). On CDO every dispatching publisher is a dependency publisher in `fixed_roles` (617 such), which by Design 6 gets the presence fold only; the 2 dispatching workspace publishers have no record parameter. A role diff over all 31,658 routines (same graph, SCCs, bindings, leaves, `fixed_roles` as analyze): 39 routines moved, every one a dependency publisher, presence facts only (`persistsCurrentRecord` 22, `mutatesParam` 31, `validatesParam` 8, `resetsFiltersOnParam` 2, `copiesIntoParam` 1, `initialisesParam` 1 no->yes; 13 parameters no->unknown on unseen dispatch); 0 non-publisher roles moved. No finding moved because those roles are read only at the raiser's call site, and every raiser of a moved publisher is Base App code whose calls are cut from the cross-app graph. Example: Sales-Post `OnAfterFinalizePosting` (Tables 110/112/114) `persistsCurrentRecord` no->yes from CDO's subscriber `src/Features/SendOnPosting/CDOSubscribers.Codeunit.al:514` (`Modify(false)` at 535-556).
- Conclusion, stated plainly: CDO does NOT exercise Design 4 (the walker enumeration) and exercises Designs 3/6 only on roles no detector reads there. The change is verified by the unit/gap rows and the r3a2/cli-a goldens, not by CDO. Census note: `params exact` counts `fixed_roles` publishers too, so it overstates walker reach; splitting it by `fixed_roles` would make that visible (final-panel minor).

## Discoveries

Filed after the merge (orchestrate step 10). All three already existed before this change, and none blocks #107.

```json
[
 {"subsystem": "l5", "locator": "src/engine/l5/detectors/d41.rs (callee role read) and src/engine/l5/detectors/mod.rs record_load_points", "symptom": "d41 and record_load_points read a bodyless (symbol-only) callee's role as given. The other readers (c1b, the walker, d37, d39) treat a callee without a body as opaque. A bodyless callee's base role says e.g. resets_filters_on_param no / persistsCurrentRecord no, which is a guess, not a fact.", "kind": "bug", "origin_issue": 107, "reproducer": "The test issue107_symbol_only_dependency_publisher_roles_unchanged (src/engine/l4/capability_cone.rs) builds a symbol-only dependency publisher whose base role reads persistsCurrentRecord no. d41 (d41.rs, which checks only the caller's body) and record_load_points (detectors/mod.rs:178, with no body_available check) read such a role as given (spec docs/superpowers/specs/2026-10-19-issue-107-publisher-roles-subscribers-design.md, probe b2).", "pre_existing": true, "capability": "Every reader of a callee role applies the same opaque guard for bodyless callees.", "acceptance": "d41 and record_load_points skip or treat as unknown any callee with body_available false; a test with a bodyless callee pins it, with a discrimination proof."},
 {"subsystem": "l4", "locator": "src/engine/l4/cfg_walker.rs apply_call: the validates && Unknown -> DirtyV rule", "symptom": "The walker's call transfer applied to an Unknown record state is order-blind for a callee that validates and then persists. Opaque(R); A(R), where A = Validate; Modify, gives DirtyV (dirtyAtExit yes), although A always persists last.", "kind": "bug", "origin_issue": 107, "reproducer": "A routine F(var R) { Opaque(R); A(R) } with A(var R) { R.Validate(..); R.Modify() } and Opaque a bodyless callee: F's dirtyAtExit is yes; the true value is no.", "pre_existing": true, "capability": "The call transfer from an Unknown state respects the callee's own exit state (persist after validate).", "acceptance": "F above gets dirtyAtExit no (or unknown), never yes; a unit row plus a discrimination proof; CDO findings measured before and after."},
 {"subsystem": "l4", "locator": "src/engine/l4/summary_runner.rs roles Jacobi over recursive SCCs", "symptom": "When an event dispatch closes a recursive SCC, every member's entry facts stay unknown. Each member starts from its base role, whose entry facts are unknown, and the join never lowers them. This is sound but imprecise: the true value is no.", "kind": "enhancement", "origin_issue": 107, "reproducer": "tests/r0-corpus ws-event-cycle: all 5 SCC members report unknown entry facts in the r3a2 summary golden.", "pre_existing": true, "capability": "The roles Jacobi starts recursive SCC members from bottom for entry facts, so a converged SCC reports what its members actually require.", "acceptance": "ws-event-cycle members report requiresLoadedAtEntry no; no other golden moves except named SCC rows; cap behaviour unchanged."}
]
```

## Not delivered
