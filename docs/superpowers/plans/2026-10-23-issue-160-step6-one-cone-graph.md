# #160 step 6: one cone graph, one scoped witness pass — plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development
> (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps
> use checkbox (`- [ ]`) syntax for tracking.

**Goal:** the view's snapshot tree is composed over `rel.cone()` from ONE composition per view
(derived rows and coverage for every routine, raw inherited facts only for primary subjects).
The cut graph, `ConeOutput`, `RAW_INHERITED_FACTS` and policy's RAW build are deleted. This
closes #161 (digest and the ordering detectors see dependency effects).

**Architecture** (spec §4):
- `WitnessScope` / `ConeScope` replace `ConeOutput`; the composition gates each member's raw
  half and returns raw apart from coverage (D1).
- `View` gains `Caps`, `Raw`, `Tree`, `Effects` cells. `prepare` composes once after `rel`,
  builds the tree (digest, ordering), computes the effects, releases the tree, then builds
  `det` from `caps` without a cone (D2, D7).
- `compose_tree` replaces `R3a3SourceBase`'s builder: cone-graph typed edges, the ledger over
  every caller (`rel.dependency_calls`), the pd arena scoped to the subjects (D4-D6).
- prove, fingerprint, diff keep a standalone single-app composition (6A-9).

**Tech stack:** Rust. Plus `scripts/agentflow` (`run`, `check-diff`), `scripts/cdo-gate`,
`scripts/bench_ab.py`, `scripts/trace_summary.py`, `/work/bench/160-10-11a/maxrss.py`, the
`cdo-measure`, `triage-findings` and `discrimination-proof` skills, the `golden-diff-triager`
agent.

**Spec:** `docs/superpowers/specs/2026-10-23-issue-160-step6-one-cone-graph-design.md`
(converged in panel round 4, 37 register entries, all accepted; "spec §n" below). Epic:
`docs/superpowers/specs/2026-10-18-issue-160-analysis-session-design.md` §9 row 6. Base
probes: `.agent/issue-160/ledger.md` `## Assumption probes`, `/work/bench/160-6/probe/`.

There are four tasks:
- **Task 1** rebases onto #220, lands the scope type and the tree builder (not yet wired), the
  measurement-only probe, and runs the stop rule's measurements. **It ends at a STOP**: the
  stop rule (spec §8.4) is read and its fallbacks fire or the claim stops, before any switch.
- **Task 2** switches digest and the ordering cell onto the view's composition and deletes the
  cut graph. One red-to-green commit.
- **Task 3** switches policy onto `Cell::Raw` and deletes `RAW_INHERITED_FACTS`. One
  red-to-green commit.
- **Task 4** is the CDO identity, triage, bench and `cdo-gate` on the final head.

Identity/comparison runs and bench runs happen only at stage boundaries (Task 1's STOP, Task
4 on the final head), per the operator's speed decision.

## Global Constraints

- **Editing.** Edit/Write tools only. No `sed -i`, `perl -pi`, heredoc or script rewrites,
  including discrimination breaks (break with Edit, revert with Edit; re-read the line to
  confirm the break applied).
- **Formatting.** `rustfmt <file>` per touched `.rs` file; never `cargo fmt`.
- **No silencing.** No `#[allow(`, no `#[ignore` in the branch. Dead code is deleted.
- **Protected paths, never edited:** `.github/`, `scripts/`, `.claude/`, `CLAUDE.md`,
  `.gitignore`, `Cargo.toml` version fields, `tree-sitter-al`. In `.agent/`, only the ledger
  rows and Timeline lines a task names.
- **Environment.** `export TREE_SITTER_AL_PATH=/work/alsem/tree-sitter-al` in every shell.
  CDO only as `CDO_WS=$(jq -er .cdo_ws /work/alsem/.agent/config.json) <command>`; the path is
  never written into a file, log name or ledger row. Timings never from the `cdo` profile.
- **Per-task gate, before the commit.**
  1. `scripts/ci-steps task-affected > <log> 2>&1`, then `grep -nE 'FAILED|^error|test result' <log>`.
     Never `| tail`.
  2. `scripts/check-goldens > <log> 2>&1`, exit status recorded. Only the moves the task lists
     may appear; each is triaged with the `golden-diff-triager` agent before
     `scripts/check-goldens --regen`. Any other move: stop and explain it.
  3. The pre-commit hook passes unmodified (never `--no-verify`).
- **After each commit,** from `/work/alsem`:
  `python3 scripts/agentflow --root /work/alsem --run-id 20261023-000639-b5e936 check-diff --base master --head HEAD --issue 160 --cwd /work/al-sem-issue-160-a7`.
  Required `reasons: []`; anything else is fixed forward in a new commit, never amended.
- **Staging.** Stage by name; never `git add -A`; never stage `.agent/`.
- **Discrimination proof** for every test a task turns green (spec §5 names each break): break
  with Edit, run the one test, record that it fails AT the named assertion, revert with Edit,
  record the pass. One ledger row per proof under `## Discrimination proofs`
  (`test | mutation | fail | pass | commit`). A break that stays green: stop and report.
- **Heavy runs and benches go through the shared slot**, output to `/work/bench/160-6/`:
  `python3 /work/alsem/scripts/agentflow --root /work/alsem --run-id 20261023-000639-b5e936 run --name bench --timeout 60 --cwd <dir> -- <command>`.
  **Quiet slot** (spec §8): a load gate (1-min load ≤ 4, wait ≤ 15 min, then fail closed) at
  the start; a load tick > 8 during a timed part voids that part (one re-run). Schedule when
  the other session runs no bench or build.
- **The licensing `cdo-gate`** runs on a COMMITTED head with a clean `src tests crates` tree:
  `CDO_WS=$(jq -er .cdo_ws /work/alsem/.agent/config.json) python3 /work/alsem/scripts/agentflow --root /work/alsem --run-id 20261023-000639-b5e936 run --name cdo-gate --timeout 60 --cwd /work/al-sem-issue-160-a7 -- bash scripts/cdo-gate`.
- **Binaries** for any measurement: `cargo build --profile release-fast --bin alsem --bin aldump`
  in a clean detached worktree under `/work/bench/160-6/`, copied out with a `bins.sha`.

## Review Focus

1. **A collided id (11b)** among the subjects: every row of the id is a subject (spec §4.3),
   the cone is per id, so the raw entry exists once. Pinned in Task 2 by T3 on the r0 corpus's
   collided fixture (if one exists; else a hand-stated two-row id in T2's fixture).
2. **A digest with no resolved root** keeps `WitnessScope::None`, builds no tree, exits 2
   (`digest_cli.rs:1682`). Task 2 adds a CLI test: `digest --routine NoSuchRoutine` exits 2
   and the trace shows `build.snapshot 0`.
3. **A routine that is both a digest root and an ordering subject** in one future batch: out
   of scope (one job per view, 6A-3); `prepare` asserts a plan has one witness request.
4. **A single-app model with body-less ABI rows** (spec §4.6): D6 may move single-app ordering
   output. Task 1 counts them; Task 4 lists every D6-explained move.
5. **An empty subject set with `needs_tree`** (an ordering plan on a workspace with no
   reportable root): the tree is built with no raw, the ordering cell is empty. Task 2 adds a
   unit test on an r0 dir whose only routine is body-less.

---

## Task 1 — rebase, scope type, tree builder, probe, and the STOP

Spec §0, §4.1, §4.4-§4.6 (builder only), §8.1 rounds rule, §8.4, §11; T2.

**Files:**
- Modify (docs-only commit, if any citation moved):
  `docs/superpowers/specs/2026-10-23-issue-160-step6-one-cone-graph-design.md`.
- Modify `src/engine/l4/cone_derived.rs`: replace `ConeOutput` with `WitnessScope` and
  `ConeScope`.
- Modify `src/engine/l4/capability_cone.rs`: `compose_inherited_cones` and
  `compose_cone_over_graph` take `ConeScope`; `ConeOutcome` gains `raw`, `cones` holds coverage
  only; every caller moves to `ConeScope` (r3a3, r3a5, the cut builder, tests) with the old
  mode's 1:1 mapping; new counters.
- Modify `src/engine/l5/detector_context.rs`: `det_cone_inputs` split; the det cone call uses
  `ConeScope` (`Both` → `{true, All}` for policy, still today's behaviour).
- Modify `src/engine/l4/exec_relation.rs`: keep `dependency_calls`.
- Modify `src/engine/l5/snapshot.rs`: add `compose_tree` (D4, D5, D6) beside the existing
  `compose_snapshot_from_base`, which stays the production path until Task 2.
- Modify `src/bin/aldump.rs` + a new `src/engine/l5/step6_probe.rs`: the measurement-only
  `--issue160-step6-probe` mode (deleted in Task 2).
- Test: T2 in `src/engine/l4/capability_cone.rs`'s test module.

**Interfaces (produced):**
- `pub enum WitnessScope { None, All, Subjects(BTreeSet<String>) }` with
  `pub fn contains(&self, id: &str) -> bool` and `pub fn is_none(&self) -> bool`
  (`cone_derived.rs`). `pub struct ConeScope { pub derived: bool, pub raw: WitnessScope }`.
- `pub fn compose_cone_over_graph(typed: &[TypedEdge], nodes: &[String], direct_full: &HashMap<String, Vec<CapabilityFact>>, coverage_in: &HashMap<String, (String, Vec<String>)>, routines: &[ModelRoutine], scope: ConeScope) -> ConeOutcome`.
- `ConeOutcome { coverage: HashMap<String, CoverageRecord>, raw: HashMap<String, Vec<CapabilityFact>>, derived: ConeDerivedStore, pd_solution: ConeArena, stats }` (`ConeResultPub` deleted).
- Counters: `build.witness_pass` (1 per composition that materialized a raw `Vec`),
  census lines `witness.members=N` and `witness.non_primary_members=N` (counted at the gate;
  role by #220's `routine_role`).
- `pub(crate) fn det_cone_inputs_over<'e>(resolved: &Model, uncertainty: impl IntoIterator<Item = &'e UncertaintyEdge>) -> DetConeInputs`; `det_cone_inputs(resolved, rel)` is the wrapper over `rel.uncertainty_all()`.
- `ExecRelation::dependency_calls(&self) -> &[CallEdge]`; census bucket `rel.dependency_calls`.
- `pub fn compose_tree(model: &Model, typed: &[TypedEdge], dependency_calls: &[CallEdge], caps: &mut CapabilityAnalysis, raw: HashMap<String, Vec<CapabilityFact>>, identities: Option<SnapshotIdentityTable>) -> CapabilitySnapshot` (`snapshot.rs`). It drains `raw` (one copy), takes `caps.pd` and drops it after the tree's arena (scoped to the subjects' state closure), records the keys `derive_typed_edges` skipped and `debug_assert!(skipped.contains(&key))` on each unmapped link (implicit-trigger exempt), and applies the `unfetched-dependency` rule in `classify_resolution`.
- `pub(crate) struct CapabilityAnalysis { pub direct_full, pub coverage, pub derived, pub pd: Option<ConeArena> }` and `pub(crate) fn compose_capabilities(model: &Model, typed: &[TypedEdge], inputs: DetConeInputs, scope: ConeScope) -> (CapabilityAnalysis, HashMap<String, Vec<CapabilityFact>>)` (`detector_context.rs`).

- [ ] **Step 1: Rebase and re-cite.** Wait for #220 (PR #229) to merge. `git fetch`; rebase
  this branch onto the post-#220 `origin/master` (the branch holds only docs commits, so the
  rebase is a replay). Re-read every **[#220]** citation of the spec and every file of its
  header list on the new base, and §11's hand-over items (a)-(e). If any citation moved or a
  claim no longer holds, fix the spec in one docs-only commit
  (`docs(#160): step 6 spec, re-cited on the post-#220 master`), `check-diff`, a ledger
  Timeline line. A claim that no longer holds and changes the design: stop and report.
- [ ] **Step 2: Parent binaries.** Build the post-#220 master in
  `/work/bench/160-6/parent220-wt` (release-fast), copy to `/work/bench/160-6/parent220/`.
  These are `P` for every number from here on.
- [ ] **Step 3: Write T2** (spec §5): R calls M calls writer W, `ConeScope { derived: true, raw: Subjects({R}) }`.
  Assert: `outcome.raw[R]` equals the `{true, All}` run's `raw[R]`; `raw` has no key M or W;
  `coverage` and the derived store have R, M and W. Run it: red (no `WitnessScope` yet:
  compile failure counts as red).
- [ ] **Step 4: Implement D1** (spec §4.1): the scope type, the per-member mode
  `(derived, raw.contains(m))` at `capability_cone.rs:2836`, `:2876`, `:2885-2898`; mode
  `(false, false)` skips the member's walk; the split output; the counters; every caller mapped
  1:1 (`DerivedOnly` = `{true, None}`, `RawOnly` = `{false, All}`, `Both` = `{true, All}`).
  T2 green.
- [ ] **Step 5: T2's discrimination proofs**: break 1, gate no-op (`raw = All`) → "no key M"
  fails; break 2, put the coverage record under the gate → "coverage has W" fails.
- [ ] **Step 6: Implement the builder half** (not wired): `det_cone_inputs_over`,
  `dependency_calls`, `CapabilityAnalysis` / `compose_capabilities`, `compose_tree`. Single-app
  equality check, test-only: on every r0 dir, `compose_tree` over the single-app graph with
  `{false, All}` equals today's `compose_snapshot` field by field (a lib test
  `compose_tree_equals_the_cut_snapshot_single_app`; break: emit a routine's drained raw
  facts before its direct facts in `derive_capability_facts` → the facts order differs on a
  fixture with ties → fails; proof row).
- [ ] **Step 7: The probe mode** `aldump --issue160-step6-probe <ws> --config <c> [--file <F>]`,
  `c ∈ {d, o, policy, parent-d, parent-o}`, printing `probe6 key=value` lines, per spec §8.4:
  `d` = `{true, Subjects(the roots of --file)}` + tree + digest effects (S4 entries and
  unresolved cones); `o` = `{false, Subjects(reportable roots)}` + tree + ordering cell;
  `policy` = `{true, Subjects(every primary)}`, no tree; `parent-d` / `parent-o` = today's cut
  base + derive + the parent's effects, and for `parent-d` the det cone (D_P). Keys: subjects,
  raw_vecs, raw_facts, non_primary_members, `k_inputs_ms`, `k_compose_ms`, `t_ms`, `e_ms`,
  `s_p_ms`, `d_p_ms`, pd_states, pd_links, pd_edge_unmapped, anchorless_with_links,
  ordering_kept_roots, dependency_calls, dependency_calls_bytes, single_app_bodyless_rows.
- [ ] **Step 8: Gate and commit** (`#160 step 6 Task 1: WitnessScope, the tree builder, the stop-rule probe`).
  The commit is byte-neutral: no golden, no `--lib` outcome, no CDO output moves (only the
  ConeOutput parity tests, rewritten on `ConeScope`, change; spec §5.9). `check-diff`, ledger
  Timeline line and proof rows.
- [ ] **Step 9: Measure in the quiet slot** (one slot job, script
  `/work/bench/160-6/task1-measure.sh`, head binaries from Task 1's commit in
  `/work/bench/160-6/task1-wt`):
  1. Parent bands: `bench_ab.py run --rounds 5` on `parent220` (A/A copy) for `d` and `o`;
     read `d`'s wall band and `o`'s `analyze.total` band.
  2. The probe, 3 rounds each through `maxrss.py`: `d`, `o`, `policy`; and 5 rounds each of
     `parent-d`, `parent-o`.
  3. The parent's whole-command peaks: `d`, `o`, cross-app `policy check` (custom policy
     `tests/fixtures/cli-c-policy/ws-policy-custom/al-sem.policy.yaml`), 3 rounds each.
  Record in the ledger (medians and bands): K, T, E′ per configuration, S_P(d), S_P(o), D_P,
  the peaks, `PD_EDGE_UNMAPPED`, `non_primary_members`, `dependency_calls_bytes`,
  single-app body-less rows, and the round rule's outcome (5 rounds if both bands ≤ 5 % of
  their medians; else 7 rounds with the trimmed band, spec §8.1).
- [ ] **Step 10: STOP — read the stop rule** (spec §8.4) and write the reading in the ledger:
  - S1: `K_d − D_P + T_d + E′_d − S_P(d) ≤ Bd`; `K_o + T_o + E′_o − S_P(o) ≤ 0.5 × Po`.
  - S2: probe peaks `d`, `o` ≤ 1.03 × parent; policy's composition peak ≤ the parent's whole
    `policy check` peak (else F13 is pre-decided for Task 3).
  - `PD_EDGE_UNMAPPED == 0` on `d` and `o` (else F6: triage the edge kind, fix the anchor
    source in Task 1's code; not closed → stop).
  - `non_primary_members == 0` on every configuration.
  - `dependency_calls_bytes` > 50 MB → F12 is pre-decided for Task 2.
  - **S1 or S2 fails on `o`** → F2: implement the three `map_op`-keyed bits and the fold-first
    walk in the same member loop (spec §10 F2) as its own commit with a unit test (a root with
    no ingredient gets no raw; break: always walk → fails), add the probe configuration `o-f2`,
    re-measure, re-read. Still failing → F11 (#228: the cross-app `caps` fill reuses the base's
    maps), re-measure, re-read.
  - **Still failing** → the claim stops at Task 1: file "step 6: cone-graph witnesses exceed
    budget on CDO" with the numbers (ledger `## Discoveries`), report to the conductor, do not
    start Task 2. No bound is relabelled.
  - **Passing** → write K, T, E′, S_P, D_P and the round rule into the ledger as the frozen
    allowance for B3, then go on to Task 2.

---

## Task 2 — digest and ordering on the view's composition; the cut graph deleted

Spec §4.2-§4.8 (except policy), §4.9, §5 T1, T3, T4, T5, T6, T7, T8 (coverage half), T9, the
live-cell checkpoint; A6.1-A6.5, A6.9-A6.11, A6.13-A6.19. Depends on Task 1 passing its STOP.

**Files:**
- Modify `src/engine/session.rs`: `Cell::{Caps, Raw, Tree, Effects}`, the fields, `Plan`'s
  `witness: WitnessRequest` and `needs_tree: bool`, the fill order of spec §4.7, the
  `view.live` census line, `prepare` asserting one witness request per plan.
- Modify `src/engine/l5/detector_context.rs`: `build_detector_artifacts` takes `caps` by move
  and composes nothing; `fill_ordering(model, &tree)`; delete the det cone call.
- Modify `src/engine/l5/digest.rs`: `compute_digest_effects_for_ordering_with` takes the tree;
  delete its `compose_snapshot`.
- Modify `src/engine/l5/digest_cli.rs`: roots from `derive_identity_table(model)` before
  `prepare`; `resolve_changed_roots(&SnapshotIdentityTable, ..)`; the query reads
  `DigestEffects`.
- Modify `src/engine/l5/snapshot.rs`, `snapshot_full.rs`: `compose_snapshot` becomes the
  standalone single-app path (asserts no dependency-role row; `build_combined_graph` +
  `det_cone_inputs_over(&graph.primary.uncertainty_edges)` + `{false, All}` + `compose_tree`);
  `compose_full_snapshot` in both universes (spec §4.4); delete `compose_snapshot_from_base`.
- Modify `src/engine/l4/capability_cone.rs`: delete `build_r3a3_source_only_base`, `_over`,
  `R3a3SourceBase`, `issue160_full_graph_timings` and the "legacy cut" text; `src/bin/aldump.rs`:
  delete the timings mode and the step-6 probe; delete `src/engine/l5/step6_probe.rs`.
- Modify `src/engine/l4/combined_graph.rs:309` doc text.
- Tests: `tests/common/dep_dispatch_fixture.rs` (the `Modify` variant), `tests/cli/` (T1, T5
  rows in `cli_build_counters.rs`, T7, T9, Review Focus 2), lib tests for T3, T4, T6, T8, the
  checkpoint and Review Focus 5; spec §5.9's rewrites (`detector_context.rs:2094`,
  `detector_context_step5_tests.rs:283`, `detector_context_11b_tests.rs:1968`,
  `capability_cone.rs:6333`, `:6371`, `tests/cli/aldump_smoke.rs:100`).
- Modify `CHANGELOG.md` (`[Unreleased]` → `### Fixed`, "#160 step 6: one cone graph", Part of
  #160, closes #161 by text only, no keyword): the snapshot tree on the cone graph; digest and
  d47/d49/d51 see dependency effects; the `unfetched-dependency` ledger rule; the deletions;
  amendments 6A-1 to 6A-11.

**Interfaces:**
- Consumes Task 1's.
- Produces: `pub enum WitnessRequest { None, ReportableRoots, Roots(BTreeSet<String>), Primary }`
  (resolved against the model inside `prepare`); `digest_plan(detectors, roots: BTreeSet<String>)`;
  `pub struct DigestEffects { pub entries: Vec<DigestEntryResult>, pub unresolved: BTreeMap<String, Vec<UnresolvedConeItem>>, pub coverage: Vec<SnapshotCoverageRecord>, pub identities: SnapshotIdentityTable }`;
  `View::effects(&self) -> &DigestEffects`; `View::raw(&self) -> &HashMap<String, Vec<CapabilityFact>>` (Task 3 reads it).

- [ ] **Step 1: Write the tests, red.** T1 (`CallChain` → DB_MODIFY on dependency Table 18 and
  COMMIT, path `CallChain → Outer → Inner`, on the `Modify` fixture variant), T3 (untouched
  `All` arm vs production scope: cli-b `c.diff` roots, dependency fixtures, r0 ordering with the
  `Hyperlink` and rid-less fixtures, CDO single-app CDO-gated arm), T4, T5 (rows of spec §4.9),
  T6, T7 (a)(b), T8 coverage equality, T9 (a)(b), the checkpoint, Review Focus 1 (a two-row
  collided id among T3's subjects: one raw entry, equal to the `All` arm's), 2 and 5. Record
  each red outcome.
- [ ] **Step 2: Implement the cells and the order** (spec §4.2, §4.3, §4.7) and the digest and
  ordering paths. Delete the det cone call. `prepare` asserts at most one `WitnessRequest`
  other than `None` per plan (Review Focus 3).
- [ ] **Step 3: Implement the standalone path and `compose_full_snapshot`; delete the cut
  builder, `compose_snapshot_from_base`, the timings mode and the probe.** `rg
  'build_r3a3_source_only_base|R3a3SourceBase|ConeOutput|issue160_full_graph|legacy cut' src`
  is empty (A6.15).
- [ ] **Step 4: Green.** All Step 1 tests pass.
- [ ] **Step 5: Goldens.** `scripts/check-goldens`. Allowed moves (A6.19): the cross-app
  digest expectations in `cli_digest_query_dependencies`; `r4_cross_app_program` and r4/r4f
  only for findings the cone graph or D6 explains. Triage each with `golden-diff-triager`,
  attributing it to the cone graph or to D6; then `--regen` and re-run. Single-app goldens must
  not move except D6-explained ordering findings (Review Focus 4), each listed.
- [ ] **Step 6: Discrimination proofs**, the breaks spec §5 names: T1 (tree over primary
  callers only), T3 (breaks 1, 2 under `Subjects` only; break 3 recorded as evidence), T4
  (`Subjects(∅)`), T5 (extra det cone), T6 (default anchor), T7 (a) omit `dependency_calls`,
  (b) keep `"resolved"`, T8 (tree folds over the cut's uncertainty), T9 (a) row removed, the
  checkpoint (skip `release(Cell::Tree)`).
- [ ] **Step 7: Gate and commit** (`#160 step 6 Task 2: digest and ordering on the cone graph (#161)`),
  `check-diff`, ledger Timeline line and proof rows.

---

## Task 3 — policy on `Cell::Raw`; `RAW_INHERITED_FACTS` deleted

Spec §4.7 policy paragraph, §4.8, T3's policy arm, T8 (materialized half), A6.6, A6.7, A6.12;
F13 if pre-decided. Depends on Task 2.

**Files:**
- Modify `src/engine/gate/policy/pipeline.rs`: delete the RAW build (`:255-272`); the rule
  engine runs after the run, off `det.summaries` and `view.raw()`; `policy_plan` gets
  `witness: WitnessRequest::Primary`, `after_run: SUMMARIES`.
- Modify `src/engine/gate/policy/policy_engine.rs`: `PolicyModel` gains
  `raw: &'a HashMap<String, Vec<CapabilityFact>>`; `select_facts(direct: &[CapabilityFact], raw: &[CapabilityFact], ..)`.
- Modify `src/engine/l5/registry.rs`, `session.rs`: delete `RAW_INHERITED_FACTS` and its
  assertion and test (`session.rs:273-276`, `:676-680`).
- Modify `src/engine/l5/full_summary.rs`, `test_support.rs`, `capability_query.rs`: the raw
  field and `inherited_raw()` become `#[cfg(test)]`; production `FullRoutineSummary::new`
  loses the raw parameter; `test_support::summary` and `cone_store_of` keep raw as fixture input;
  delete `capability_query.rs:295`.
- Rewrite `tests/cli/cli_events_policy_dependencies.rs:439` on `Cell::Raw`.
- Tests: T3 policy arm (selected facts per primary routine against the parent's
  `inherited_raw()` path, recorded as a fixture file before the switch), T8 break 1.
- `CHANGELOG.md`: add the policy line to Task 2's entry.

**F13 (only if Task 1 pre-decided it, or B5 later misses):** the rule engine runs right after
the composition, before `det`; `PolicyModel.summaries` is a temporary
`HashMap<String, FullRoutineSummary>` built by COPYING `caps.direct_full` and `caps.coverage`
for the primary routines only (decision: copies, not a borrowed-slices refactor of
`PolicyModel`; primary direct facts are small next to raw, and the copy dies with the rule
engine). Then `Raw` is released and `det` takes `caps`.

- [ ] **Step 1: Record the oracle fixture** on Task 2's head: for every r0 dir and the
  dependency fixtures, the facts each policy rule selects per primary routine (today's path).
- [ ] **Step 2: Write T3's policy arm and T8 break 1's assertion** (`witness.non_primary_members == 0`
  on a cross-app policy run of the dependency fixture). Red: the policy arm fails to compile
  against the new `select_facts`.
- [ ] **Step 3: Implement** the files above. Green. `rg 'RAW_INHERITED_FACTS' src` empty;
  `inherited_raw` only under `#[cfg(test)]`.
- [ ] **Step 4: Discrimination proofs:** T8 break 1 (policy scope `All` → count > 0 → fails);
  T3 policy arm (drop `raw` from `select_facts`'s `inherited` arm → fails).
- [ ] **Step 5: Gate and commit** (`#160 step 6 Task 3: policy reads the witness pass`),
  `check-diff`, ledger lines. `scripts/check-goldens` must not move (cli-c-policy unchanged).

---

## Task 4 — CDO identity, triage, bench, cdo-gate (the final head)

Spec §7, §8, A6.3, A6.4, A6.7, A6.17, A6.20, E14; B1-B5; F6, F8-F13.

**Files:** create `docs/measurements/2026-10-<dd>-issue-160-step6.md`. Code only if a
fallback fires (its own commit, tests, proofs, gate, `check-diff`).

- [ ] **Step 1: Head binaries** from the final head in `/work/bench/160-6/final/head-wt`;
  parent = `/work/bench/160-6/parent220/`. Update `/work/bench/160-6/bench-draft.sh` into
  `/work/bench/160-6/final/bench.sh` for 6A-10 (B3 rule with the frozen allowance), B5, the
  round rule from Task 1, D6 attribution in Part 1.
- [ ] **Step 2: Part 1 (identity and moves)**, one slot job: histogram SHA equal; SARIF `cmp`
  default analyze both universes and scopes; single-app d47/d49/d51 `cmp` except listed D6
  moves; `policy check` custom and bundled `cmp` (A6.7; F7 if not); single-app fingerprint
  `cmp`; cross-app `o` SARIF and `d` JSON diffed; the census lines (`non_primary_members=0`,
  §4.10 buckets); `PD_EDGE_UNMAPPED == 0` (F6).
- [ ] **Step 3: Triage** (`cdo-measure` comp, `triage-findings`): every moved d47/d49/d51
  finding, attributed to the cone graph or D6, and the digest coverage/effects deltas. > 30 %
  FP → F8 (root-cause fix first).
- [ ] **Step 4: Parts 2 and 3 (bounds)** in the quiet slot, through `agentflow run --name bench`:
  read B1-B5 per spec §8.1 (void checks first; one re-run on void; F9 on void twice; F10 on a
  miss, one fix from F2, F11, F12 or F13).
- [ ] **Step 5: The licensing `cdo-gate`** on the final committed head; exit 0.
- [ ] **Step 6: Commit the numbers file** (`docs(#160): step 6 CDO measurement, triage and bench`):
  Task 1's stop-rule reading, Part 1-3 results, the bounds, the triage table, fallbacks fired,
  the `cdo-gate` record. `check-diff`, ledger `## Gate results` rows.

---

## Acceptance map (spec §6)

| Acceptance | Where |
|---|---|
| A6.1, A6.9 | Task 2 (T1) |
| A6.2 | Task 2 (T8 coverage half) |
| A6.3, A6.4 | Task 1 STOP (S1, S2), Task 4 Step 4 (B1-B4) |
| A6.5 | Task 2 (T5) |
| A6.6 | Task 3 (T8 break 1), Task 4 Step 2 (census) |
| A6.7 | Task 4 Step 2 |
| A6.8 | Task 1 Step 10 |
| A6.10, A6.11 | Task 1 (T2), Task 2 (T3) |
| A6.12 | Task 3 (T3 policy arm) |
| A6.13 | Task 2 (T9, T3 digest arm) |
| A6.14 | Task 2 (T4) |
| A6.15 | Task 2 Step 3, Task 3 Step 3 |
| A6.16 | Task 2 (T7) |
| A6.17 | Task 2 (T6), Task 4 Step 2 |
| A6.18 | Task 2 (checkpoint) |
| A6.19 | Task 2 Step 5 |
| A6.20 | Task 4 Step 3 |
| E14 | Task 4 Step 5 |
| B5 (policy peak) | Task 4 Step 4; F13 in Task 3 if pre-decided |
