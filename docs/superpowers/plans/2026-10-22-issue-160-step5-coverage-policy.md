# #160 step 5: one coverage policy — plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development
> (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps
> use checkbox (`- [ ]`) syntax for tracking.

**Goal:** every producer of direct coverage applies the one four-kind fold. The detector context
(`det_cone_inputs`) and the cross-app base (`build_cross_app_base`) fold both uncertainty halves.
The test-only L4-solve fold is deleted. Every reader that moves is pinned and triaged on CDO.

**Architecture** (spec §4):
- `callsite_local_coverage_reasons` takes any edges.
- `ExecRelation::uncertainty_all()` chains the Primary half and the dependency half.
- P-a and P-b fold per row before the projection, as P-c and P-d already do.
- P-c and P-d keep folding their own graph's edges (the snapshot stays on the cut graph until
  step 6).

**Tech stack:** Rust. Plus `scripts/agentflow` (`run`, `check-diff`), `scripts/cdo-gate`, the
`cdo-measure` and `triage-findings` skills.

**Spec:** `docs/superpowers/specs/2026-10-22-issue-160-step5-coverage-policy-design.md`
(converged in panel round 4, 23 register entries, all accepted; "spec §n" below). Epic:
`docs/superpowers/specs/2026-10-18-issue-160-analysis-session-design.md` §9 row 5. Probes and
the prototype numbers: `.agent/issue-160/ledger.md`, `/work/bench/160-5/`.

There are two tasks:
- **Task 1** is the code, one red-to-green commit. Every acceptance test of spec §5 turns green
  in it.
- **Task 2** is the CDO measurement and triage of spec §8. Its numbers decide whether a
  pre-stated fallback (F1, F2, F6) adds commits.

Identity and comparison runs happen only at stage boundaries: on Task 1's head, and on the
final head after any fallback. Step 5 is non-speed (Decision 58): no bench.

## Global Constraints

- **Editing.** Use the Edit/Write tools only. No `sed -i`, `perl -pi`, heredoc or script
  rewrites, including discrimination breaks (break with Edit, revert with Edit). Re-read the
  edited line to confirm a break applied before running it.
- **Formatting.** `rustfmt <file>` per touched `.rs` file; never `cargo fmt`.
- **No silencing, no skipping.** No `#[allow(` and no `#[ignore` in the branch (`check-diff`
  forbids both). Dead code is deleted.
- **Protected paths, never edited:** `.github/`, `scripts/`, `.claude/`, `CLAUDE.md`,
  `.gitignore`, `Cargo.toml` version fields, `tree-sitter-al`. In `.agent/`, only the ledger
  rows and Timeline lines a task names.
- **Environment.**
  - `export TREE_SITTER_AL_PATH=/work/alsem/tree-sitter-al` in every shell.
  - CDO only through a per-command prefix, with the path read at run time:
    `CDO_WS=$(jq -er .cdo_ws /work/alsem/.agent/config.json) <command>`.
  - The path is never written into a file, log name or ledger row.
  - Timings are never quoted from the `cdo` profile.
- **Per-task gate, before the commit.**
  1. `scripts/ci-steps task-affected > <log> 2>&1`, then
     `grep -nE 'FAILED|^error|test result' <log>`. Never `| tail`.
  2. `scripts/check-goldens > <log> 2>&1`, exit status recorded. Spec A4: green with NO regen.
     A golden that moves is F5: stop and explain it with the `golden-diff-triager` agent.
  3. The pre-commit golden hook passes unmodified (never `--no-verify`).
- **After each commit,** from `/work/alsem`:
  `python3 scripts/agentflow --root /work/alsem --run-id 20261022-213444-4c7bee check-diff --base master --head HEAD --issue 160 --cwd /work/al-sem-issue-160-a6`.
  Required: `reasons: []`. Anything else is fixed forward in a new commit, never amended.
- **Staging.** Stage the task's files by name; never `git add -A`. Never stage `.agent/`.
- **Discrimination proof** for every test a task turns green (spec §5 names each break):
  1. Break with Edit.
  2. Run the one test and record that it fails AT the named assertion.
  3. Revert with Edit, run, record the pass.

  Write one ledger row per proof under `## Discrimination proofs`:
  `test | mutation | fail output | pass output | commit`. A break that comes back green is
  evidence about the test: stop and report it.
- **Heavy runs go through the shared slot:**
  `python3 /work/alsem/scripts/agentflow --root /work/alsem --run-id 20261022-213444-4c7bee run --name bench --timeout 60 --cwd <dir> -- <command>`,
  with output to a log file under `/work/bench/160-5/`.
- **The licensing `cdo-gate`** runs on a COMMITTED head while nobody edits the tree.
  `git -C /work/al-sem-issue-160-a6 status --porcelain -- src tests crates` must be empty before
  and after. Command:
  `CDO_WS=$(jq -er .cdo_ws /work/alsem/.agent/config.json) python3 /work/alsem/scripts/agentflow --root /work/alsem --run-id 20261022-213444-4c7bee run --name cdo-gate --timeout 60 --cwd /work/al-sem-issue-160-a6 -- bash scripts/cdo-gate`.

### The acceptance tests

Another agent is writing the spec §5 tests in this worktree now (uncommitted, in `src/` and
`tests/`): T1, T1b, T2, T3, T4 (with `write_fixture_dep_ambiguous` in
`src/engine/l5/test_support.rs`), T5, T6, T7, T8, T9 (main and control variants).
- All of them are red on the parent and turn green in Task 1. Nothing is held back.
- A test's body changes only if it contradicts the spec. Each change gets a ledger row with
  the spec clause.
- The F1/F2/F6 variant tests (spec §10, §5 "T9 if F6 fires") are written only if their
  fallback fires, in that fallback's commit.

## Review Focus

1. **Collided ids** (11b). The fold runs per row BEFORE `merge_per_id` / `least_complete`, at
   the same point in P-a and P-b as in P-c and P-d (`capability_cone.rs:4407`, `:4620`).
   Pinned by T3 (equality with the snapshot) and T5 (P-a == P-b). A collided r0 id is covered
   by T3 when one exists.
2. **Single-app keeps its meaning.** `rel.dependency_uncertainty()` is empty single-app, so
   `uncertainty_all()` there is exactly `primary().uncertainty_edges`. Pinned by T3 (single-app).
3. **The policy RAW build** (`gate/policy/pipeline.rs:266`) goes through `det_cone_inputs` too,
   so policy and analyze read one coverage. Pinned by T2 (production `policy check`).
4. **An edge whose `from` has no row** is ignored, as today. 0 on CDO (P1).
   `fold_uncertainty_reasons` is looked up per row id, so this needs no code.
5. **Order.** The scan builds a `BTreeSet` per routine, so the order of the two halves cannot
   move output (spec §4.4). Covered by T4/T5 (both halves present).

---

## Task 1 — the one fold (code, tests, deletion)

Spec §4.1-§4.5, §5, A1-A5, A10, 5-A4, 5-A5.

**Files:**
- Modify `src/engine/l4/capability_cone.rs`:
  - `callsite_local_coverage_reasons` (`:3639-3648`) becomes
    `pub(crate) fn callsite_local_coverage_reasons<'e>(edges: impl IntoIterator<Item = &'e UncertaintyEdge>) -> HashMap<&'e str, BTreeSet<&'e str>>`.
    Its doc (`:3618-3638`) keeps the "the solve never moves the four kinds" argument, now
    pointing at `db_effect_solver.rs:1025`, `:1103`, `:1143`, and drops every mention of the
    deleted function.
  - `fold_uncertainty_reasons` (`:3654-3673`) becomes `pub(crate)`; body unchanged.
  - P-c (`project_r3a3`, `:4391`) and P-d (`build_r3a3_source_only_base_over`, `:4607`) call
    the scan with `&graph.uncertainty_edges`.
  - P-b (`build_cross_app_base`): call the scan once with `rel.uncertainty_all()` after `rel` is
    built (`:3906`) and before the per-row loop (`:3950`). Fold each row with
    `fold_uncertainty_reasons(status, reasons, extra.get(r.id.as_str()))` before
    `coverage_per_row.push`. Drop the scan's map before `rel` moves into the base.
  - Delete `compute_uncertainty_coverage_reasons` with its doc (`:3546-3616`).
  - Delete the tests that pin it: `check_oracle` (`:6331`) with its helpers used only by it,
    `callsite_local_scan_matches_the_fold_on_fixtures` (`:6477`),
    `callsite_local_scan_matches_the_fold_on_cdo` (`:6568`) and
    `callsite_local_scan_matches_the_fold_under_a_roles_cap` (`:6630`).
  - Rewrite the test-module doc (`:6132`) so it no longer names the deleted function.
  - Keep `callsite_local_scan_hand_stated`, `snapshot_base_folds_the_scan_into_direct_coverage`
    and `snapshot_base_prints_no_capped_line`, adjusting only the scan's argument.
- Modify `src/engine/l4/exec_relation.rs`:
  - Add `pub fn uncertainty_all(&self) -> impl Iterator<Item = &UncertaintyEdge>` (Primary half,
    then dependency half). Its doc is spec §4.1's.
  - Rewrite the "no production reader" doc lines (`:38-41`, `:132`) to name the fold.
- Modify `src/engine/l5/detector_context.rs`:
  - `det_cone_inputs(resolved: &Model, rel: &ExecRelation) -> DetConeInputs`: scan
    `rel.uncertainty_all()` once before the per-row loop, fold each row before
    `coverage_per_row.push`.
  - Its call in `build_detector_artifacts` (`:1362`) passes `rel`.
- Update the test callers of `det_cone_inputs` to pass a relation:
  - `detector_context.rs:2449`;
  - `detector_context_11b_tests.rs:169`, `:340`;
  - `capability_cone.rs:6944`, `:6962`, `:7259` (`check_model` already has `rel`);
  - the single-app ones build `ExecRelation::single_app(&m)`.
  The step-6 agent's files are not edited beyond this mechanical argument.
- Modify `CHANGELOG.md`, under `[Unreleased]` → `### Fixed`, "#160 step 5: one coverage policy"
  (Part of #160, no closing keyword). Content:
  - The detector context and the cross-app base now fold the four call-site-local kinds, both
    uncertainty halves (#161 part 2).
  - The readers that move: spans/d9, digest's and prove's `transactionContext`, d35 UNKNOWN,
    d45 text, fanout's `capabilityComposition` and `--coverage-policy strict`, the policy
    `complete` gate.
  - d2 can lose a finding through its walk budget (T9).
  - The L4-solve fold and its four tests are deleted.
  - Amendments 5-A1 to 5-A8.

**Interfaces:**
- Produces:
  - `ExecRelation::uncertainty_all(&self) -> impl Iterator<Item = &UncertaintyEdge>`;
  - `det_cone_inputs(&Model, &ExecRelation) -> DetConeInputs`;
  - `callsite_local_coverage_reasons<'e>(impl IntoIterator<Item = &'e UncertaintyEdge>) -> HashMap<&'e str, BTreeSet<&'e str>>`;
  - `fold_uncertainty_reasons` as `pub(crate)`.

- [ ] **Step 1: Red.** Confirm the step-6 tests exist and compile. Run them and record that each
  fails at the assertion spec §5 names: T1 (`coverage_complete`), T1b (`transactionContext`),
  T2 (counters), T3 (equality), T4 (`Go` complete), T6, T7, T8, and T9's main variant (the d2
  finding IS emitted). T5 compiles only after Step 2's signature change, so record its first
  run after Step 2 (with P-b's fold not yet added, it fails on `Inner`).
- [ ] **Step 2: Implement the scan, the accessor and the P-a fold** (files above). Run
  `cargo test -p al-sem --lib det_cone_inputs` and T1, T3, T4: green. T5 is still red.
- [ ] **Step 3: Implement the P-b fold.** T5 green.
- [ ] **Step 4: Delete the L4-solve fold and its four tests; rewrite the two docs.**
  `grep -rn compute_uncertainty_coverage_reasons src` gives 0 hits (A10).
- [ ] **Step 5: Discrimination proofs**, one per test, with the break spec §5 names:
  - T1, T1b, T2, T3, T6, T7, T8, T9 (main): delete the fold call in `det_cone_inputs`.
  - T4: scan `rel.primary().uncertainty_edges` only.
  - T5: drop the fold call in `build_cross_app_base`.
  - T9 control: same break, and the finding is still emitted (a control; record it, it is not
    a discrimination).
  One ledger row each.
- [ ] **Step 6: Gate.** `rustfmt` each touched file; `scripts/ci-steps task-affected`;
  `scripts/check-goldens` (no regen, A4/A5's single-app half). Any golden move → F5 stop.
- [ ] **Step 7: Commit** the Task 1 files plus the step-6 agent's test files
  (`#160 step 5 Task 1: one coverage fold for every producer (#161 part 2)`). Then
  `check-diff` on the new HEAD, a ledger Timeline line and the proof rows.

---

## Task 2 — CDO measurement, triage, and the pre-stated fallbacks

Spec §7, §8, §10, A5-A9. Depends on Task 1's commit.

**Files:**
- Create `docs/measurements/2026-10-22-issue-160-step5.md`: the numbers below, the triage table
  and the fallback record.
- Code files only if a fallback fires (listed under each fallback).

- [ ] **Step 1: Head binaries.**
  - `git -C /work/alsem worktree add --detach /work/bench/160-5/head-wt <Task 1 SHA>`.
  - Build `cargo build --profile release-fast --bin alsem --bin aldump` in the slot.
  - Copy both binaries to `/work/bench/160-5/head/`.
  - The parent binaries are already in `/work/bench/160-5/parent/` (`f96619ec`).
- [ ] **Step 2: Comparison runs, parent vs head** (one slot job; a script beside
  `run-proto.sh`, writing under `/work/bench/160-5/final/`):
  1. `python3 /work/alsem/.claude/skills/cdo-measure/scripts/measure.py run --bin <side> --label before|after --out /work/bench/160-5/final/measure`,
     then `comp`. Expected (prototype): changes only in d35 and d9, plus their
     `detectorStats`. Fingerprint bytes equal (A5).
  2. d45 explicitly at both scopes and `--single-app` (as `run-d45.sh`). Expected 150 → 150,
     text only.
  3. `analyze --single-app` both sides. Expected d35 6 → 67, d9 27 → 37.
  4. `events fanout --format json --deterministic`, both universes. Diff
     `capabilityComposition` per event.
  5. `policy check`, bundled (byte-identical required, A9) and `ws-policy-custom`, both
     universes. Report the deltas of findings and `routinesSkippedCoverage`.
  6. `aldump --program-call-graph-stats`: SHA equal to `c032c8db…` (A6).
  Every number that differs from the prototype's (spec §0) is explained before going on.
- [ ] **Step 3: The span probe** (spec §8 step 4).
  - Write a CDO-gated `#[ignore]` lib test in `/work/bench/160-5/head-wt` only (never on the
    branch), modelled on `/work/bench/160-5/step5_probe.rs`.
  - It prints every transaction span's `(seed kind, commit routine id, coverage_complete)`,
    single-app and cross-app.
  - Run it on the head worktree, and on a detached parent worktree with the same probe file.
  - For every span that flipped:
    - `alsem digest <WS> --routine <commit routine> --format json --deterministic` (both
      universes) and `alsem prove <WS> <commit routine> may-commit --deterministic`, parent vs
      head;
    - the diff must be only `transactionContext`;
    - digest's `coverage` key must be byte-equal (A5, cross-app half).
  - Remove both worktrees afterwards; keep the outputs.
- [ ] **Step 4: Triage** (the `triage-findings` skill, spec §7's verdicts).
  - Cover every added d35 and d9 finding of the default mode at both scopes, the flipped fanout
    events, the changed spans, and the d45 text changes. Single-app gets a seeded sample of 30
    (seed recorded).
  - Work root cause first: one hub, one verdict per uncertain call, by kind
    (`interface-open-world` by the interface's `Access`; `ambiguous-overload` false;
    `external-target` true; `member-not-found` by existence). Then check each finding of the
    hub.
  - Use a span's reasons as spec §7 defines them.
  - Record epistemic and hazard verdicts side by side, per detector, universe and scope.
  - Gating (A8): d35 UNKNOWN and d9's coverage-admitted findings, default mode, ≤ 30 %
    epistemically false at each scope. Fanout, `transactionContext` and d45 are reported, not
    gated.
  - F3 applies if any detector has more than 300 adds in one mode and scope: a seeded sample of
    30.
- [ ] **Step 5: Fallbacks, only those whose trigger fired**, each its own commit with its own
  tests, discrimination proofs, gate and `check-diff`. Spec §10 is binding; summary:
  - **F1** (d35 > 30 %):
    - first, file one issue per systematic cause (an `Internal` interface emitted as
      open-world; a compiler-decided `ambiguous-overload`), with its hub as repro;
    - then the opt-in id `d35u-commit-may-be-reachable-in-event-subscriber` takes the UNKNOWN
      findings whose reasons are non-empty and all fold kinds;
    - mixed-reason and empty-reason UNKNOWN findings stay in d35, with the same id and
      fingerprint, and their text may gain the fold reasons;
    - tests: T7 under `--detector d35u-…`, absent under the default selection and under
      `--detector d35`; the mixed-reason test; `cli_d35_unknown_commit` stays green.
    - Files: `src/engine/l5/detectors/d35.rs`, `src/engine/l5/detectors/mod.rs` (register in
      `OPT_IN_DETECTORS`), tests in the `cli` umbrella.
  - **F2** (d9 > 30 %): the same split for d9 into `d9u-…`. A finding moves only if its span is
    incomplete, it was admitted by coverage alone (fewer than 2 pending physical tables), and
    its reasons are non-empty and all fold kinds. Add the complete-span preservation test.
  - **F6** (the head lost a d2 finding on CDO, or a d2 r4 golden moved):
    - confirm each loss is a `NodeBudgetCut` caused by a newly entered `Unknown` branch;
    - d2's `expand` orders admitted edges `Yes` first, `Unknown` after, stable within each
      class;
    - T9 becomes T9-main (mutation: remove the ordering → lost) and T9-nested (mutation: delete
      the fold → emitted);
    - re-run Step 2's comparison: every lost d2 finding must be restored (same id and
      fingerprint);
    - each residual becomes a stated limit with a repro in the numbers file and spec §7, plus
      one discovery issue "d2: replace the budgeted `walk_evidence` with budget-free
      reachability, as d1 did". The fold stays.
  - **F5** (any unpredicted golden or test move): stop and explain it first.

  After any fallback commit, re-run Step 2 on the new final head (one comparison per stage
  boundary, not per edit).
- [ ] **Step 6: The licensing `cdo-gate`** on the final committed head (Global Constraints
  command). Required: exit 0 (A7).
- [ ] **Step 7: Commit the numbers file** (`docs(#160): step 5 CDO measurement and triage`).
  It holds:
  - every command line and both sides' `meta.json`;
  - the comp output;
  - the d45, single-app, fanout, policy, histogram and span-probe results;
  - the triage table with FP rates;
  - the fallbacks fired and the issues filed;
  - the `cdo-gate` record;
  - "no bench (Decision 58)".

  Then `check-diff`, and a ledger Timeline line plus `## Gate results` rows. Discovery issues
  go into the ledger's `## Discoveries`.

---

## Acceptance map (spec §6)

| Acceptance | Where |
|---|---|
| A1 | Task 1 (T3) |
| A2 | Task 1 (T4, T5) |
| A3 | Task 1 (T1, T1b, T2, T6, T7, T8, T9); d1 by spec §3.2's stated limit |
| A4 | Task 1 Step 6 and every fallback commit (`check-goldens`, no regen) |
| A5 | Task 1 Step 6 (single-app goldens); Task 2 Step 2.1 (fingerprint) and Step 3 (cross-app digest `coverage`) |
| A6 | Task 2 Step 2.6 |
| A7 | Task 2 Step 6 |
| A8 | Task 2 Steps 4-5 |
| A9 | Task 2 Step 2.5 |
| A10 | Task 1 Step 4 |
