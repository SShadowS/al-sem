# Issue #265 — d35 unknown answer: name the subscriber's own unresolved call site(s)

## Claim

```json
{"run_id":"20261024-133535-cd8e4c","issue":265,"attempt":1,"session":"claude-code-session:57653578-e111-4b15-8684-017371f816fa","branch":"issue/265-agent-discovery-l5-d35-message-a1","worktree":"/work/al-sem-issue-265-a1","body_hash":"2969e7a7a29a30ec3d4a5cc04a2adb6b86b33e0365e79f38488deb2f3b9e0617","reconciled":[]}
```

Issue body hash (claim `body_hash`): `2969e7a7a29a30ec3d4a5cc04a2adb6b86b33e0365e79f38488deb2f3b9e0617`.
Base (`make-worktree`): `70c3b7438e91a64fe8ddcb9698f093d9b926a0bd` (master after #264).
Reviewers (operator instruction, confirmed for this queue): `gpt-6.1-sol` in the astra slot, Opus in the flash slot; attest with `--substitute astra=sol --substitute flash=opus`. Marks stored as `reviews: {sol: …, opus: …}`.
Operator (2026-10-24, "i confirm"): queue #264 → #265 → #271 → #272. Claimed with a one-off `--loop-max 3` (2 used since the reset).
Hold (alsem-main, 2026-10-24 ~13:35, digest timing): no release builds, cdo-gate or CDO runs until it signals; spec work and dev-profile tests are fine.

## Classification

**Bounded.** A message-only change inside `src/engine/l5/detectors/d35.rs`: everything it needs is already on `DetectorContext` when d35 builds the message (the subscriber's own uncertainty edges in `uncertainty_edges_by_from`, each with a `callsite_id`; `call_site_by_id` → `callee_text` + `source_anchor`; the `pub(crate)` kind filter `l4::db_effect_solver::is_callsite_local_kind`). No l4 or cone change.

## Assumption probes

Run by the design author (opus), read-only, no CDO (timing hold).

| probe | result | evidence |
|---|---|---|
| where the cone records why coverage is partial | per routine `direct_facts_for_routine` (unknown + opaque-dependency / parse-incomplete), else partial when the folded reason list is non-empty; `fold_uncertainty_reasons` adds one reason per call-site-local edge keyed by its `from`; `coverage_cone_for_scc` unions reasons and puts every SCC member whose direct status is partial/unknown plus successors' targets into `unknown_targets`. The call-site id is dropped there | capability_cone.rs:520-537, :1111, :2425-2472, :3572-3606; detector_context.rs:1292-1297 |
| is the call-site identity available to d35 | yes: every call-site-local edge has `callsite_id: Some(..)` (interface-open-world directly; the other three via `simple_uncertainty_cs`); the edges sit ungated in `uncertainty_edges_by_from` (from `rel.primary()`); `call_site_by_id` gives `callee_text`, `source_anchor`; d13 and d16 already use it | combined_graph.rs:377-433; detector_context.rs:858, :1607-1616, :1079-1082; d13.rs:98 |
| line base | `PAnchor.start_line` is 0-based; display code adds 1 | ir_walk.rs:296; format_html.rs:193; format_pr_summary.rs:150 |
| why the subscriber is in its own `unknown_targets` | only its own partial direct status, i.e. its own call-site-local edges for a d35 candidate; exceptions: (a) another row with the same id is opaque (`least_complete` merge), (b) an edge's site id is missing from `call_site_by_id`; `object-run-unresolved` adds a reason, never a target | capability_cone.rs:2433-2445; detector_context.rs:1299-1300; d35.rs:54-58 |
| the direct repro has the edge | `i264_a3_extensible_true_stays_open` asserts an `interface-open-world` edge from ReleaseOnRegistered | cli_d35_unknown_commit.rs:700-731, :866-880 |
| several sites / own + deeper | today one merged reason set and one target set (self, SCC partners, successors' targets); edges can repeat per call site → dedup by site id | capability_cone.rs:2431-2466 |
| other readers of `unknown_targets` / `reasons` | fingerprint_query.rs:334-377, :1047, :1544-1585; snapshot.rs:2766; cone_census.rs:221; test_coverage.rs:173; aldump:565; step5 tests — none change (data untouched) | grep |
| goldens pinning d35 unknown text | none; fingerprints do not hash message text (`hash_parts` = detector, object, routine name, tables, key) | grep tests/ (empty); fingerprint.rs:249-259 |
| tests pinning the text | only cli_d35_unknown_commit.rs:552 (direct repro) changes; :541 (IsDue), :364 (symbol-only), :243 (V1), cli_step5_coverage_fold.rs:398 and the d35 unit tests stay | grep `partly analysed` |
| base green | 24/24 `cli_d35_unknown_commit::` at 70c3b743 | cargo test |

## Design

**Goal.** When a d35 unknown answer comes from gaps in the subscriber's own body, name those call sites (callee text and line), and list only OTHER routines under "partly analysed".

**Scope (revision 2, both reviewers).** Own sites are named for PRIMARY (workspace) subscribers, in single-app and default cross-app mode. In cross-app mode a subscriber that lives in a DEPENDENCY app has its own edges only in `rel.dependency_uncertainty` (exec_relation.rs:80-84, :141-146), which is not on `DetectorContext`; `uncertainty_edges_by_from` is primary-only (detector_context.rs:1607-1616) and stays so (other detectors read it). Such a subscriber (reported at `--scope all`) keeps today's text: no regression, goal not met there → stated limit and Discovery D1.

**Non-goals.** No l4 or cone change; no per-site evidence steps or anchors (SARIF) — separate change; no reason kind per site; the `alsem fingerprint` text (which also lists the routine itself) is unchanged; the yes/direct arms, confidence, severity, id and fingerprint are unchanged; dependency subscribers (above).

**Design.** `coverage_reasons` also takes `routine: &ModelRoutine`. The early returns stay exactly as today ("no effect summary", "coverage unknown"), with no own-site segment.
1. Own sites: for each uncertainty in `ctx.uncertainty_edges_by_from[routine.id]` whose kind passes `is_callsite_local_kind`, look up its `callsite_id` in `ctx.call_site_by_id`; collect into a `BTreeMap` keyed by `(start_line, start_column, cs.id)` (dedup by site, source order, distinct sites with equal text both kept); value `"{callee_text} (line {start_line + 1})"`, where `callee_text` has every whitespace run collapsed to one space (it is raw source and may span lines).
2. Self-drop only when EVERY own gap is accounted for: (a) at least one own site was named, (b) every own call-site-local edge resolved to a site (none missing from `call_site_by_id`), and (c) no other model row with the subscriber's routine id is opaque or parse-incomplete (`least_complete` merge, projection.rs:210; capability_cone.rs:525-538). `ctx.routine_by_id` keeps only the last row per id (detector_context.rs:1068-1071), so d35 computes, ONCE per run (not per subscriber), the set of routine ids that have an opaque or parse-incomplete row, in one pass over the workspace (and in cross-app mode, the dependency) routine rows. Otherwise `routine.id` stays under "partly analysed" (honest fallback). Targets otherwise as today (display-sorted, deduped); self is filtered BEFORE display mapping and the cap.
3. Output: `reasons` + `"; unresolved: "` + up to 3 sites + `", +N more"`, then `"; partly analysed: "` + up to 3 OTHER targets + `", +N more"`; each segment only when non-empty; one shared helper prints at most N items plus `+N more` (both `SHOWN_TARGETS = 3`).
4. Label: "unresolved:" is the issue's own wording. The module doc states that it means "a call whose callee set is not closed for this proof" (interface-open-world, ambiguous-overload, external-target, ...), not the `unresolved-call` uncertainty kind, which is filtered out.
5. Docs: d35 module doc, `SHOWN_TARGETS` doc, CHANGELOG `Changed` entry.

Expected direct-repro text (line counted by hand; confirm on the first run): `Check what could not be analysed (interface-open-world; unresolved: Strategy.NextKm (line 11)); if a Commit is reachable, move it out of the subscriber or defer it.` The same `{reasons}` string feeds rootCause and the evidence note.

**Acceptance matrix.**

| # | item | test | break that must fail it |
|---|---|---|---|
| A1 | direct repro names `Strategy.NextKm (line 11)`, not ReleaseOnRegistered | repin `centralgauge_repro_direct_interface_call_is_unknown_at_medium` (deliberate) | remove the self-drop → ReleaseOnRegistered back; print without `+1` → line 10 |
| A2 | IsDue path still names IsDue | `centralgauge_repro_is_unknown_at_medium` unchanged | drop all targets |
| A3 | own gap + deeper routine: both segments | NEW cli `centralgauge_direct_and_isdue_names_site_and_isdue` (subscriber calls `Strategy.NextKm` AND `DemoPublisher.IsDue`) → `…; unresolved: Strategy.NextKm (line N); partly analysed: IsDue (Codeunit 50103)`; asserts the d35 finding only (not the `bySeverity` totals); run in default cross-app AND `--single-app` | remove the self-drop |
| A4 | several sites: source order, identity, dedup, kind filter, cap | NEW unit `own_sites_source_order_dedup_and_more`: call-site-local sites out of alphabetical order incl. two DISTINCT sites with the same `callee_text` on different lines and two sites on the same line with different columns; one site's edge twice (adjacent AND non-adjacent); an `unresolved-call` uncertainty (real, has a site id, not call-site-local) on another site → exact output: first 3 in (line, column) order and `+N more` with the exact remainder, both equal-text sites counted, no `unresolved-call` site | sort by callee text; drop the kind filter; key by text instead of site id (loses one equal-text site) |
| A5 | self-drop + line base + whitespace (unit) | NEW unit `own_site_named_and_self_dropped`: hand-built callee text `"Strategy\n    .NextKm"` (the test first asserts it contains `'\n'`) → `…; unresolved: Strategy .NextKm (line 11); partly analysed: helper (Codeunit 1)` (a collapsed run becomes ONE space); asserts rootCause, evidence note and fixHint surfaces | as A1; no whitespace collapse → newline in output |
| A6 | no site found → self kept | NEW unit `own_gap_without_site_keeps_self` | drop self unconditionally |
| A6b | found + missing site → self kept, found site still named | NEW unit `own_gap_partly_located_keeps_self` (one edge resolves, one site id missing) → `unresolved: X (line n)` AND `sub (Codeunit 1)` under partly analysed | drop self when ≥1 site named (the revision-1 rule) |
| A6c | named site + opaque or parse-incomplete same-id row → self kept | NEW unit `own_site_with_incomplete_sibling_keeps_self` (two cases: opaque sibling, parse-incomplete sibling; the sibling rows are not d35 candidates (d35.rs:54-59), so the test asserts exactly one finding) | drop the sibling check |
| A6d | dependency subscriber (stated limit) | NEW cli `dependency_subscriber_keeps_todays_text` (cross-app, `--scope all`, subscriber in a source dependency with an own interface gap). Preconditions asserted first: a d35 finding exists for the dependency subscriber, and it is listed under "partly analysed". Then: no `unresolved:` segment. Needs a scope argument (`analyze_with` hard-codes `Scope::Primary`, cli_d35_unknown_commit.rs:154) | n/a (pins the limit; would move only if D1 is done) |
| A7 | no other text moves | V1, symbol-only, step5_t7, `no_summary_*`, `coverage_none_*`, d35 unit tests green; check-goldens green | n/a |

**Failure shapes.** An unreachable site is still named (matches the reason; the fold does not filter it either). A long chained callee text makes a long message (accepted; F1). SCC partners' gaps stay under "partly analysed" (they are other routines). Collided ids: global `call_site_by_id` lookup, last row wins; a missing site keeps self (A6b). A same-id opaque or parse-incomplete row keeps self (A6c), so its gap is not hidden.

**Pre-stated fallbacks.** F1: if real code shows unreadably long callee texts, cut the callee text at 60 characters (char-boundary safe) with `…`, always keeping the ` (line n)` suffix. F2 (revision 2): a dependency subscriber in cross-app mode keeps today's text (stated limit, A6d); there is no in-context alternative read — the data is `rel.dependency_uncertainty()`, not on `DetectorContext` (Discovery D1).

**Measurement.** None required (finding count, ids, fingerprints unchanged). Optional later: a `cdo-measure` before/after confirming zero fingerprint churn and counting d35 unknown findings that gain `unresolved:`.

Possible discovery: the `alsem fingerprint` text also lists the routine itself under "N opaque/unresolved targets" (fingerprint_query.rs:334-377) — #160 owns fingerprint_query.rs, so not touched here.
Discovery D1 (revision 2): expose the dependency half of the uncertainty edges (`rel.dependency_uncertainty()`) to detectors, so d35 can name own sites for a dependency subscriber at `--scope all`.

## Timeline

- 13:36 claim #265 attempt 1, worktree at `70c3b743`. Probes + design dispatched (opus).
- 13:42 classified bounded; design in `## Design`.
- Spec panel r1: sol approve-with-fixes (S1 scope, S2 attribution important; S3 minor), opus approve-with-fixes (O1 important; O2–O8 minor). Revision 2.
- Spec panel r2: sol approve (11/11); opus approve-with-fixes (11/11 + O9, O10 minor). Revision 3.
- Spec panel r3: both approve (S1, S2, O1, O5, O8, O9, O10 accepted). **Converged**: 13 entries, all `fixed`, all accepted by sol and opus. spec_rounds 3 of 5. Opus wording note for the implementer: in cross-app mode `workspace.routines` already includes the dependency rows (the coverage merge's row set, detector_context.rs:1286-1306), so §2(c) is one pass over it.
- Plan (1 task) committed `39b37357`; plan_tasks charged 1. Bounded with one task → no per-task reviewer (the final panel is the review).
- Task 1 (opus) `233e0de3`, 1 attempt: tests red then green (d35 unit 12/12, cli_d35_unknown_commit 26/26); `task-affected` whole suite 4152/4152; check-diff clean. Direct-repro text: `Check what could not be analysed (interface-open-world; unresolved: Strategy.NextKm (line 11)); if a Commit is reachable, move it out of the subscriber or defer it.` F1 not needed.
- ci-steps-task at `233e0de3` exit 0 (42 s, supervised; `logs/ci-steps-task/20261024T141523-8d59d395.log`). Draft PR #286.
- Final round 1 (final_rounds 1 of 5): sol approve (no findings); Opus approve (one minor: `incomplete_ids` built on every run → deferred); code-review skill at high (7 unverified candidates, `final-r1-codereview.md`): CR1 important (own sites looked up in global `call_site_by_id`; collided `#if` subscriber rows print each other's site) → fix via the routine's own `call_sites`; CR3 (raw callee text collapsed → render from `PCallee` like the snapshot) → fix; CR4 (unreachable sites named) → don't name, keep self, discovery for the cone; CR6 (A3 evidence-note surface) → fix; CR2 (label) refuted (spec O4); CR5 (dependency limit) refuted (spec S1/O1, D1); CR7 deferred (= Opus minor). Fixes dispatched to the implementer.
- Round-1 fixes: `38dabc29` FC1, `8b0fda7f` FC3, `28a6235f` FC4, `5621e450` FC6 (each red before its fix). ci-steps-task at `5621e450` exit 0 (`logs/ci-steps-task/20261024T143540-bd4523d0.log`), pushed.
- Final round 2 (final_rounds 2 of 5): Opus approve (7/7 accepted; FN1 line wraps); sol accepted FC1, FC4, FC6, FC2/FC5 refutations, FC7 deferral, re-raised FC3 (a multi-line raw receiver keeps its newline).
- `13933506` FC3/FN1: whitespace collapsed after `display_of_callee` (unchanged), `multi_line_receiver_prints_on_one_line` red before; CHANGELOG qualified and rewrapped. ci-steps-task exit 0 (`logs/ci-steps-task/20261024T144441-e35acf56.log`), pushed.
- Final round 3 (final_rounds 3 of 5): both approve, FC3 and FN1 accepted. **Final panel converged**: 21 entries; all fixed except FC2, FC5 refuted (both accepted the refutation) and FC7 deferred (minor); all accepted by sol and opus.
- Next: step 11 full gates — HELD (alsem-main timing window: no release builds, cdo-gate or CDO runs until it signals). Lifted ~14:55 ("Go ahead with #265's gates").
- Step 11: origin/master unmoved → no rebase. **B = `70c3b7438e91a64fe8ddcb9698f093d9b926a0bd`, H = `139335062c935f099f47ec5c8daef6d95d18be1f`.** Full set started.

## Discrimination proofs

| test | mutation | fail output | pass output | commit |
|------|----------|-------------|-------------|--------|
| A5 `own_site_named_and_self_dropped`, A1 `centralgauge_repro_direct_interface_call_is_unknown_at_medium`, A3 `centralgauge_direct_and_isdue_names_site_and_isdue` | self-drop removed | A5 `…partly analysed: helper (Codeunit 1), sub (Codeunit 1)`; A1 `…unresolved: Strategy.NextKm (line 11); partly analysed: ReleaseOnRegistered (Codeunit 50104)`; A3 `…IsDue (Codeunit 50103), ReleaseOnRegistered (Codeunit 50104)` | all ok | `233e0de3` |
| A5, A1 | line printed without `+1` | `(line 10)` | ok | `233e0de3` |
| A5 | no whitespace collapse | `unresolved: Strategy\n    .NextKm (line 11)` | ok | `233e0de3` |
| A4 `own_sites_source_order_dedup_and_more` | sort by callee text | `Alpha (line 10), Alpha (line 3), Beta (line 6), +2 more` | ok | `233e0de3` |
| A4 | kind filter dropped | `Unres (line 2), Alpha (line 3), Beta (line 6), +3 more` | ok | `233e0de3` |
| A4 | key by callee text instead of site id | `…Yak (line 8), +1 more` (one equal-text site lost) | ok | `233e0de3` |
| A6 `own_gap_without_site_keeps_self` | self always dropped | `…(interface-open-world); a Commit…` (self missing) | ok | `233e0de3` |
| A6b `own_gap_partly_located_keeps_self` | self dropped once ≥1 site named (revision-1 rule) | `…(interface-open-world; unresolved: X (line 5)); …` | ok | `233e0de3` |
| A6c `own_site_with_incomplete_sibling_keeps_self` | sibling check removed | `…unresolved: X (line 5)); …` (opaque case) | ok | `233e0de3` |
| A6c (parse-incomplete half) | incomplete set keeps only `!body_available` | parse-incomplete case fails | ok | `233e0de3` |
| A2 `centralgauge_repro_is_unknown_at_medium` | all targets dropped | `Check what could not be analysed (interface-open-world); if…` | ok | `233e0de3` |

Red phase (before implementation): A1, A3, A4, A5, A6b, A6c red; A6 and A6d already green (A6: today's code keeps self when no site; A6d pins the stated limit, preconditions held: one d35 finding on `HandleRaised`, fixHint lists `HandleRaised (Codeunit 70011)`). A6d and A7 are controls with no break.

## Findings register summary

`findings.json`: 21 entries. Spec panel (3 rounds): S1–S3 (sol), O1–O10 (opus) — all `fixed`. Final panel (3 rounds; code-review skill at high + sol + opus): FC1, FC3, FC4, FC6 (code-review skill) and FN1 (opus) `fixed`; FC2, FC5 `refuted` (settled in the spec panel, both reviewers accepted the refutation); FC7 `deferred` (minor, cost only). Every entry accepted by sol and opus.

## Gate results

| gate | head | exit | log |
|---|---|---|---|
| ci-steps-task | `233e0de3` | 0 (42 s) | `.agent/runs/20261024-133535-cd8e4c/logs/ci-steps-task/20261024T141523-8d59d395.log` |
| ci-steps-task | `5621e450` | 0 (34 s) | `.agent/runs/20261024-133535-cd8e4c/logs/ci-steps-task/20261024T143540-bd4523d0.log` |
| ci-steps-task | `13933506` | 0 (42 s) | `.agent/runs/20261024-133535-cd8e4c/logs/ci-steps-task/20261024T144441-e35acf56.log` |
| **ci-steps-all** (H) | `13933506` | **0** (382 s, supervised) | `.agent/runs/20261024-133535-cd8e4c/logs/ci-steps-all/20261024T144744-1129e498.log` |
| **check-goldens-coverage** (H) | `13933506` | **0** (supervised) | `.agent/runs/20261024-133535-cd8e4c/logs/check-goldens-coverage/20261024T145406-b672fc2c.log` |
| **check-goldens** (H) | `13933506` | **0** (tree stamp trusted; supervised) | `.agent/runs/20261024-133535-cd8e4c/logs/check-goldens/20261024T145406-6bc9057f.log` |
| **cdo-gate** (H) | `13933506` | **0** (470 s, supervised) | `.agent/runs/20261024-133535-cd8e4c/logs/cdo-gate/20261024T145406-241a5329.log` |

Tree clean after the full set. Grammar c89c2ecb; the pinned CDO workspace @ bc3ccb18.

## Discoveries

```json
[
  {
    "subsystem": "l5 detector context (dependency-half uncertainty edges)",
    "locator": "src/engine/l5/detector_context.rs:1607-1616 (uncertainty_edges_by_from is primary-only); src/engine/l4/exec_relation.rs:80-84, :141-146 (dependency_uncertainty)",
    "symptom": "In the default cross-app mode a d35 subscriber that lives in a DEPENDENCY app (reported at --scope all) gets no own-site names in its unknown answer and lists itself under 'partly analysed', because the dependency half of the uncertainty edges is not on DetectorContext.",
    "kind": "enhancement",
    "origin_issue": 265,
    "reproducer": "tests/cli/cli_d35_unknown_commit.rs dependency_subscriber_keeps_todays_text: Dep Sub.HandleRaised (a dependency subscriber with an own interface gap) at --scope all reads 'partly analysed: HandleRaised (Codeunit 70011)' with no 'unresolved:' segment.",
    "pre_existing": true,
    "capability": "Detectors can read the dependency half of the uncertainty edges (rel.dependency_uncertainty()) through DetectorContext, keyed by from like uncertainty_edges_by_from, without changing the primary-only index other detectors use.",
    "acceptance": "dependency_subscriber_keeps_todays_text is updated deliberately: the dependency subscriber's message names its own site (callee and line) and no longer lists itself; primary-subscriber output unchanged; no detector reading uncertainty_edges_by_from changes."
  },
  {
    "subsystem": "l4 combined graph / capability cone (unreachable call sites)",
    "locator": "src/engine/l4/combined_graph.rs (uncertainty edges emitted with no unreachable filter); src/engine/l4/capability_cone.rs:566 (direct_facts_for_routine skips unreachable sites), :3573-3606 (fold_uncertainty_reasons)",
    "symptom": "A call site in unreachable code (control_context 'unreachable', e.g. after exit;) still emits a call-site-local uncertainty edge, which the coverage fold counts as a partial gap. A d35 subscriber whose only gap is dead code therefore gets an unknown finding; since #265 it lists itself instead of naming the dead site.",
    "kind": "bug",
    "origin_issue": 265,
    "reproducer": "A d35 subscriber with `exit;` followed by a dead `Strategy.NextKm(1);` on an interface over an extensible enum, and no other gap: alsem analyze reports a d35 unknown finding with interface-open-world and 'partly analysed: <subscriber>'.",
    "pre_existing": true,
    "capability": "Uncertainty edges (or the coverage fold) ignore call sites whose control_context is unreachable, consistent with direct_facts_for_routine.",
    "acceptance": "On the reproducer d35 gives no unknown finding (the cone is complete); reachable sites unchanged; the CDO histogram and finding set are measured before/after and any move triaged."
  }
]
```

## Not delivered

All acceptance items delivered (A1–A7, A6b–A6d). Stated limit: a dependency subscriber in cross-app mode keeps today's text (A6d, Discovery D1). Known rendering limits (FC3): the receiver part of a callee is raw source (whitespace collapsed; a comment inside it can survive on one line); quoted method names print unquoted. FC7 (lazy build of the incomplete-id set) deferred.
