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

**Split (spec §14, amendment 6A-12, 2026-10-23).** Task 1's stop rule failed (ledger
`## Task 1 STOP`). Step 6 now moves **digest only** onto the cone graph; the ordering cell
(d47/d49/d51) and the cut builder stay until a #161 follow-up, and one BFS predecessor-DAG
witness rule (spec §14.3) replaces the per-fact search. The Goal and Architecture above are
narrowed accordingly: the cut graph is NOT deleted, #161 is closed for digest only, and
`RAW_INHERITED_FACTS` goes only if Task 3 stays. Where this plan's Task 1 text and the tasks
below disagree, the tasks below win.

The tasks:
- **Task 1** (done, STOPPED): the scope type and the tree builder (not wired), the probe, the
  stop-rule measurements. Commit `72abd499`, cherry-picked at attempt 8 as `edca9f0c`.
- **Task 1′** (claim time, no code on the branch): rebase, re-measure with the DAG prototype,
  read S1′/S2′. STOP if either fails.
- **Task 2a**: the DAG witness rule, on today's trees (every consumer). One red-to-green
  commit that may move witness goldens. Nothing else moves.
- **Task 2b**: digest on the view's one composition over the cone graph. One red-to-green
  commit.
- **Task 3**: policy on `Cell::Raw` (kept only if B5 holds in Task 4).
- **Task 4**: CDO identity, triage, bench, `cdo-gate`, numbers file.

Splitting the old Task 2 into 2a and 2b keeps each golden move attributable to one cause: 2a's
moves are the DAG rule, 2b's are the cone graph or D6.

Identity/comparison runs and bench runs happen only at stage boundaries (Task 1′'s STOP, Task
2a's early CDO check, Task 4 on the final head), per the operator's speed decision.

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
  `python3 scripts/agentflow --root /work/alsem --run-id 20261023-120326-46d656 check-diff --base origin/master --head HEAD --issue 160 --cwd /work/al-sem-issue-160-a8`
  (attempt 8; attempt 7 used run `20261023-000639-b5e936` and `/work/al-sem-issue-160-a7`,
  which every run id and worktree below now replace).
  Required `reasons: []`; anything else is fixed forward in a new commit, never amended.
- **Staging.** Stage by name; never `git add -A`; never stage `.agent/`.
- **Discrimination proof** for every test a task turns green (spec §5 names each break): break
  with Edit, run the one test, record that it fails AT the named assertion, revert with Edit,
  record the pass. One ledger row per proof under `## Discrimination proofs`
  (`test | mutation | fail | pass | commit`). A break that stays green: stop and report.
- **Heavy runs and benches go through the shared slot**, output to `/work/bench/160-6/`:
  `python3 /work/alsem/scripts/agentflow --root /work/alsem --run-id 20261023-120326-46d656 run --name bench --timeout 60 --cwd <dir> -- <command>`.
  **Quiet slot** (spec §8): a load gate (1-min load ≤ 4, wait ≤ 15 min, then fail closed) at
  the start; a load tick > 8 during a timed part voids that part (one re-run). Schedule when
  the other session runs no bench or build.
- **The licensing `cdo-gate`** runs on a COMMITTED head with a clean `src tests crates` tree:
  `CDO_WS=$(jq -er .cdo_ws /work/alsem/.agent/config.json) python3 /work/alsem/scripts/agentflow --root /work/alsem --run-id 20261023-120326-46d656 run --name cdo-gate --timeout 60 --cwd /work/al-sem-issue-160-a8 -- bash scripts/cdo-gate`.
- **Binaries** for any measurement: `cargo build --profile release-fast --bin alsem --bin aldump`
  in a clean detached worktree under `/work/bench/160-6/`, copied out with a `bins.sha`.

## Review Focus

1. **A collided id (11b)** among the subjects: every row of the id is a subject (spec §4.3),
   the cone is per id, so the raw entry exists once. Pinned in Task 2b by T3 on the r0 corpus's
   collided fixture (if one exists; else a hand-stated two-row id in T2's fixture).
2. **A digest with no resolved root** keeps `WitnessScope::None`, builds no tree, exits 2
   (`digest_cli.rs:1690` at `6a2b7c8b`). Task 2b adds a CLI test: `digest --routine NoSuchRoutine`
   exits 2 and the trace shows `build.snapshot 0`.
3. **A routine that is both a digest root and an ordering subject** in one future batch: out
   of scope (one job per view, 6A-3); `prepare` asserts a plan has one witness request.
4. **A single-app model with body-less ABI rows** (spec §4.6): D6 may move single-app ordering
   output. Task 1 counts them; Task 4 lists every D6-explained move. *Since the split:* D6 is on
   the cone-graph tree only and ordering stays on the cut, so it cannot move ordering output;
   Task 1 measured 0 single-app body-less rows.
5. **An empty subject set with `needs_tree`** (an ordering plan on a workspace with no
   reportable root). *Void since the split*: the ordering plan has no witness scope.

---

## Task 1 — rebase, scope type, tree builder, probe, and the STOP

**Done; STOPPED at Step 10** (S1 and S2 failed on `d` and `o`, ledger `## Task 1 STOP`).
Code commit `72abd499` (attempt 7) = `edca9f0c` (attempt 8 cherry-pick). Kept as written for
the record; Task 1′ continues from here.

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

## Task 1′ — rebase and re-measure (claim time)

Spec §14.4, §14.8, §14.9. No code on the branch.

- [ ] **Step 1: Rebase.** Done at attempt 8 by cherry-picking the parked branch (`10266644`
  ..`2a4a867f`) onto master `6a2b7c8b`: the one textual conflict in `capability_cone.rs` was
  resolved by keeping Task 1's `ConeOutcome` doc and dropping the #221-era `ConeResultPub`
  (Task 1 = `edca9f0c`); #78's compile break in `step6_probe.rs` is fixed in its own commit
  (`6a94aeac`, `OwnerIndex::build(model)`; `--file` roots never read it). `ci-steps
  task-affected` on `6a94aeac`: 3812/3812 pass. Still to run on that tree before Step 3:
  `scripts/check-goldens > <log>` and `scripts/ci-steps clippy`; both must be byte-neutral.
- [x] **Step 2: Spec commit.** The amendment is spec §14 (`4eca8a3b`, `check-diff` clean).
  Its spec-panel round runs before Step 3.
- [ ] **Step 3: Parent binaries** from `6a2b7c8b` in a clean detached worktree
  (`/work/bench/160-6/parent6a2-wt` → `/work/bench/160-6/parent6a2/`, `bins.sha`).
- [ ] **Step 4: Probe binaries**: the branch head plus the rule-B prototype patch from
  `/work/bench/160-6/diag-wt` (`git -C /work/bench/160-6/diag-wt diff > /work/bench/160-6/split/proto.patch`;
  it touches `digest.rs`, `ordering_engine.rs`, `ordering_inter.rs`, `step6_probe.rs` and installs mimalloc in
  `aldump.rs`), plus P-2's census arm (`digest.effects`, `digest.ordering`, DAG, merge window per root).
  Scratch worktree only, never the branch. Every probe run sets `MIMALLOC_PURGE_DELAY=0` (spec §14.6).
- [ ] **Step 5: Run** `/work/bench/160-6/split/probes-todo.md` P-1 to P-5 in one slot job: `d` and the
  multi-root `h` (`src/Features/HandledState/CDOUnhandledSalesPagesMgt.Codeunit.al`), 3 rounds each, plus
  the parent's whole-command `d` and `h` peaks and walls on `alsem`; P-4 (triage of every changed CDO effect on
  the cut) and P-5 (corpus golden diff of the DAG rule) are REQUIRED. A round with a load tick > 8 is void and
  re-run (A1-20). Record medians, bands and the round rule in the ledger.
- [ ] **Step 6: STOP — read** S1′ (`d`: `K − D_P + T + E′ − S_P ≤ 8,000 ms`), S2′ (`d` probe peak
  ≤ `Pm(d)` + 3,145,728 KiB) and S3′ (`h`: E′ ≤ 60,000 ms and probe peak ≤ 12,582,912 KiB).
  `PD_EDGE_UNMAPPED == 0` and `non_primary_members == 0` on `d` and `h`. Pass → freeze the terms in the ledger
  and go on. Fail → F-W1 or F-W2 once (as a scratch probe first), re-read; still failing → **D** (spec §14.8):
  digest stays on the cut, Task 2b is not done, Task 2a and Task 3 go on.

---

## Task 2a — the DAG witness (D8, spec §14.3)

Acceptance: A6.21, A6.22 (corpus half), A6.19 (DAG half). Depends on Task 1′ passing its STOP.

**Files:**
- Modify `src/engine/l5/digest.rs`: `reconstruct_witness_paths`'s BFS half becomes the DAG rule
  (spec §14.3.1). A new per-root `struct DagCache<'i>(HashMap<(FirstHopKind, String), Rc<WitDag<'i>>>)`
  beside `HopCache` (which stays an edge → hop memo). The reverse-BFS `valid_nodes` set is replaced by a
  carrier set per `ValidNodesKey` (rename the cache to what it holds). Paths are enumerated by the ordered
  forward walk over marked DAG nodes, up to the CALLER's `cap + 1` (spec §14.3.1 item 4). Opaque seeds as
  today. The per-fact depth rule (item 6). `MAX_STATES` stays for `enumerate_proofs` only. The streaming
  top-k merge (§14.3.1a) replaces `all_paths`/`all_path_conds`; the old `merge_normalize_via_paths` stays
  `#[cfg(test)]` as W10's oracle. Census line `digest.witness_dags=N nodes=N preds=N`. Port from the
  prototype WITHOUT its `ALSEM_W6_*` switches and `diag` module; the old search is deleted, kept only as a
  `#[cfg(test)]` oracle if a W test needs it.
- `reconstruct_witness_paths_pub` takes `&mut HopCache` and `&mut DagCache`; `fingerprint_query.rs:560-600`
  holds one pair per root (spec §14.3.1 item 3).
- Modify `src/engine/l5/ordering_engine.rs` / `ordering_inter.rs`: the on-demand happens-before test and the
  shared `Arc<[OrderedOp]>` group ops (spec §14.3.7). `build_hb_edges` stays (test oracle of O1).
- Tests in `digest.rs`'s test module (hand-stated `FingerprintIndexes` over a literal snapshot,
  as the existing witness tests do). Each states its precondition literally and gets a proof row:
  - **W1 shortest only.** Root R, first hop `cs1` → A; A → W1 (carrier) and A → B → W2
    (carrier of the same fact). Expect one path, to W1. Break: read every reached carrier
    instead of those at `dmin` → two paths → fails.
  - **W2 all shortest.** `cs1` → X; X → A → W and X → B → W. Expect both paths. Break: keep
    only the first predecessor per routine (the CLRS breadth-first tree) → one path → fails.
  - **W3 one DAG per (root, first hop).** Two facts of R through `cs1`, one through `cs2`:
    `witness_dags` = 2. Break: skip the cache lookup → 3 → fails.
  - **W4 truncation boundary, at the caller's cap.** A ladder of `k` diamonds under `cs1` (2^k
    shortest paths): at cap 256, `k = 8` (256 paths) → `truncated == false`, `k = 9` → `true`; at
    cap 3 (the fingerprint query's `--witness 3`), 3 paths → false, 4 → true, `path-limit-reached
    cap=3`. Break: enumerate up to `cap` instead of `cap + 1` → fails; hard-code 256 → the cap-3
    arm fails.
  - **W4b ordered prefix.** More than `cap` shortest paths over two carriers and reconverging
    diamonds: the returned paths are the first `cap` by (length, JSON) of ALL of them (computed by
    brute force in the test). Break: enumerate in predecessor order → fails.
  - **W5 no state budget.** A fan-out that makes the old search expand more than 25,000 states
    before the carrier: the witness is found and there is no `state-limit-exceeded`
    diagnostic. Break: stop the DAG BFS after 25,000 pops → fails.
  - **W6 depth limit.** The only carrier sits 65 hops below the first hop: today's depth-limit
    diagnostic and `incomplete`, not `terminal-not-found`. Break: never set the depth flag →
    fails.
  - **W6b depth is per fact.** (a) A carrier at 2 hops plus an unrelated 70-hop branch: no
    diagnostic. (b) Two facts; only fact B's carrier is past 64: fact A carries no diagnostic.
    Break: set the diagnostic whenever the DAG hit the depth frontier → fails.
  - **W7 conditionality over shortest witnesses.** A guarded shortest path plus an unguarded
    longer one → `conditional` (spec §14.3.1b). Break: read conditionality over all paths
    (re-run the old search for it) → `unconditional-on-success` → fails.
  - **W8 one DAG per root in the fingerprint query.** A fingerprint query over two facts of one
    root with one first hop builds 1 DAG. Break: a fresh `DagCache` per call → 2 → fails.
  - **W9 opaque seeds.** An opaque call-site seed is reported and counts toward `cap + 1`; a
    non-seed opaque routine reached by the DAG is not reported. Break: report every opaque node
    the DAG reaches → fails.
  - **W10 streaming merge = old merge.** For every effect of every corpus root, the streaming
    merge's printed paths, `viaPathsTruncated` and conditionality equal the old merge's (kept
    test-only); plus a hand-stated input with equal-JSON paths of different conditionality and a
    path evicted then offered again. Break: keep `MAX_PATHS` instead of `MAX_PATHS + 1` in the
    window → truncation differs → fails.
  - **O1 on-demand happens-before.** For every ordered pair of occurrences of every corpus
    root's groups, the on-demand test equals membership in `build_hb_edges`'s output, duplicate
    occurrence ids included. Break: drop the `!(…)` guard → fails.
- `CHANGELOG.md` `[Unreleased]` → `### Changed`: "digest, prove and fingerprint witnesses are
  the shortest paths to the nearest carrier (one BFS per root and first hop, CLRS §22.2);
  printed paths are no longer padded with longer ones; conditionality is read over the shortest
  witnesses; the witness BFS has no state limit, and the depth diagnostic is per fact; the
  ordering engine answers happens-before on demand (no change in d47/d49/d51 output)". Part of
  #160.

- [ ] **Step 1: Write W1-W10, W4b, W6b, O1, red** (W1, W5 and W6b fail on the old search; W3, W6,
  W8 fail to compile without the new counter and cache). W2, W4, W10 and O1 can pass before the change:
  they pin properties of the new code, and their breaks (Step 5) are what proves them. Record each
  outcome.
- [ ] **Step 2: Implement** the rule, the streaming merge and the on-demand happens-before test. All
  green.
- [ ] **Step 3: Goldens.** `scripts/check-goldens > <log>`. Allowed moves: spec §14.3.4's allowlist
  (`viaPaths`, `viaPathsTruncated`, `conditionality`, the witness diagnostics, occurrence and fact ids
  that hash the via paths, prove answers and blockers that read truncation or conditionality, the
  fingerprint query's witness output). Not allowed: `tests/r4-goldens/`, `tests/r4f-goldens/`,
  `r4_cross_app_program`, any `.cbor`/`.cbor.gz`/shard, any `objectDisplay` or header, effect
  types/details/evidence. Triage with `golden-diff-triager` (classes: padding removed, truncation flip,
  conditionality flip, recovered witness, id moved with its via path, prove answer moved; other =
  unexplained), then `--regen` and re-run.
- [ ] **Step 4: Early CDO check (slot job, ~2 min):** `o` SARIF `cmp` parent vs this head,
  cross-app at both scopes and `--single-app` (A6.22). Any difference → F-W3.
- [ ] **Step 5: Discrimination proofs** for W1-W10, W4b, W6b, O1 (break with Edit, revert with
  Edit, re-read the line).
- [ ] **Step 6: Gate and commit** (`#160 step 6 Task 2a: the DAG witness (one BFS per root and first hop)`),
  `check-diff`, ledger lines.

---

## Task 2b — digest on the view's composition over the cone graph

Spec §4.2-§4.7 for digest only, §4.9 as amended (§14.10), T1 (digest half), T3 (digest arm),
T4, T5, T6, T7, T8 (coverage half), T9, the live-cell checkpoint; A6.1, A6.2, A6.5, A6.10
(digest), A6.13-A6.19. Depends on Task 2a.

**Files:**
- `src/engine/session.rs`: `Cell::{Caps, Raw, Tree, Effects}`; `Plan` gains
  `witness: WitnessRequest` and `needs_tree: bool`;
  `pub enum WitnessRequest { None, Roots(BTreeSet<String>), Primary }` (no `ReportableRoots`:
  6A-6 is void); fill order of spec §4.7; the `view.live` census line; `prepare` asserts at most
  one non-`None` witness request. Digest needs the model before the composition: add
  `View::prepare_model(&plan)` (cells 1-2) and let `prepare` continue from there, or an
  equivalent split; state the choice in the commit.
- `src/engine/l5/detector_context.rs`: `build_detector_artifacts` takes `caps` by move and
  composes nothing (delete the det cone call). **`fill_ordering` and its cut snapshot are
  unchanged.** Policy's RAW build keeps working through a command-local composing builder
  (`compose_policy_raw(model, rel) -> (DetectorArtifacts, …)`, today's body over `det_cone_inputs` with
  `ConeScope { derived: true, raw: All }`), deleted by Task 3 (spec §14.5, A1-13).
- `src/engine/l5/digest.rs`: rule B on digest's cone-graph tree only (spec §14.3.5): the tree carries
  `boundary_witnesses: bool`; with it on, witness paths end at the first dependency routine with the
  `enters <App>.<Routine>` callsite terminal, and boundary-only effects are keyed by (effect type,
  boundary call site) with the group `detail` (`boundary`, `resourceCount`, `resources`: up to 10, in
  resource-id order, display when present).
- `src/engine/l5/digest_cli.rs`: before the composition, in one block that ends before
  `prepare`: `derive_identity_table(model)`, `OwnerIndex::build(model)`,
  `resolve_changed_roots(&identities, model, &input, primary_guid_of(model), &owners)` (the #78
  signature, with the table instead of the snapshot). At (8): `OwnerIndex::build(view.model())`
  again and `run_digest_query_full_from_entries(&effects, .., &changed_roots, &owners)`. No
  resolved root → `WitnessRequest::None`, `needs_tree: false`, exit 2.
- `src/engine/l5/snapshot.rs`: `compose_tree` becomes digest's production path.
  `compose_snapshot` and `compose_snapshot_from_base` STAY (ordering cross-app; prove,
  fingerprint, diff single-app); their doc says which readers remain and names #161. No
  single-app assertion.
- `src/engine/l4/capability_cone.rs`: delete `issue160_full_graph_timings` only; rewrite the
  "legacy cut" text (`build_r3a3_source_only_base`'s doc) to name its one cross-app reader, the
  ordering cell, and #161. `src/engine/l4/combined_graph.rs:300` (the "legacy cut" doc line at
  `6a2b7c8b`) the same.
- `src/bin/aldump.rs`: delete the timings mode and `--issue160-step6-probe`; delete
  `src/engine/l5/step6_probe.rs` and `tests/cli/aldump_smoke.rs`'s timings test.
- `snapshot_full.rs`: unchanged (`compose_full_snapshot` stays on the cut base until step 9,
  spec §14.7).
- Tests: `tests/common/dep_dispatch_fixture.rs` (`Modify` variant), `tests/cli/` (T1, T5 rows,
  T7, T9, Review Focus 2), lib tests (T3 digest arm, T4, T6, T8 coverage half, the checkpoint,
  Review Focus 1); spec §5.9's rewrites except the ones on the cut builder that now stays
  (`detector_context.rs:2094`, `capability_cone.rs` cut-builder tests keep their subject).
- `CHANGELOG.md` `### Fixed`: "#160 step 6: digest sees dependency effects (one cone graph, #161 digest
  half)" with spec §14.3.6's reader note (grouped boundary effects, `enters <App>.<Routine>`, resource
  count and up to 10 resources, witness paths stop at the boundary); the `unfetched-dependency` ledger rule
  on the digest tree; the timings mode deleted; d47/d49/d51 stay on the cut graph (follow-up under #161),
  with its I5 exception stated.

**Interfaces:** consumes Task 1's and 2a's. Produces `WitnessRequest` (above); `digest_plan(detectors,
roots: BTreeSet<String>)`;
`pub struct DigestEffects { pub entries: Vec<DigestEntryResult>, pub unresolved: BTreeMap<String, UnresolvedCone>, pub coverage: Vec<SnapshotCoverageRecord>, pub identities: SnapshotIdentityTable }`
where `UnresolvedCone` is today's result of `unresolved_cone` (items AND `UnresolvedTraversal`, A1-16);
`View::effects(&self) -> &DigestEffects`; `View::raw(&self) -> &HashMap<String, Vec<CapabilityFact>>` (Task 3 reads it).

- [ ] **Step 1: Tests red.** T1 (digest, grouped shape: `CallChain` reports `DB_MODIFY` and `COMMIT` as
  groups through the call into `Dep Lib.Outer`, Customer (18) in the `DB_MODIFY` group's resources), B1-B4
  (spec §14.3.5), T3 digest arm, T4, T5 (spec §14.10 rows: digest 1/1/1/2/1/1/1/1; d47 row unchanged,
  witness 1; the two-composition analyze row), T6, T7 (a)(b), T8 coverage equality (on `d`'s fixture, `h`
  shape and policy; not `o`), T9 (a)(b), the traversal block test (A1-16), checkpoint, Review Focus 1 and
  2; and the policy check: Task 2b alone keeps the policy goldens and CDO `policy check` byte-identical
  (A1-13).
- [ ] **Step 2: Cells, order, digest path.** Delete the det cone call.
- [ ] **Step 3: Deletions** (timings mode, probe). `rg 'ConeOutput|issue160_full_graph|issue160_step6' src`
  empty.
- [ ] **Step 4: Green.**
- [ ] **Step 5: Goldens.** Allowed: cross-app digest expectations in
  `cli_digest_query_dependencies` (cone graph, D6, rule B's grouped effects). Everything single-app and
  every r4/r4f golden must not move (2a already took the DAG moves; rule B is off on the cut). Triage,
  regen, re-run.
- [ ] **Step 6: Discrimination proofs**: T1 (tree over primary callers only), B1-B4 (spec §14.3.5), T3
  (breaks 1, 2 under `Subjects`; break 3 as evidence), T4 (`Subjects(∅)` on `compose_tree`'s caller), T5
  (extra det cone), T6, T7 (a)(b), T8, T9 (a), the traversal block test (drop `UnresolvedTraversal`
  from `DigestEffects` → fails), checkpoint.
- [ ] **Step 7: Gate and commit** (`#160 step 6 Task 2b: digest on the cone graph (#161, digest half)`),
  `check-diff`, ledger.

---

## Task 3 — policy on `Cell::Raw`; `RAW_INHERITED_FACTS` deleted (conditional)

Spec §4.7 policy paragraph, §4.8, T3's policy arm, T8 (materialized half), A6.6, A6.7, A6.12;
F13 if pre-decided. Depends on Task 2b. `witness: WitnessRequest::Primary`. **Kept only if B5
holds** (spec §14.5): its own commit so Task 4 can revert it cleanly. F13 only if Task 4's B5
misses. If B5 still misses after F13: `git revert` the Task 3 commit (and the F13 commit),
re-run the per-task gate, file the follow-up; `RAW_INHERITED_FACTS`, policy's RAW build and
`inherited_raw` stay, and A6.7/A6.12 read "policy unchanged".

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
- `CHANGELOG.md`: add the policy line to Task 2b's entry.

**F13 (only if Task 1 pre-decided it, or B5 later misses):** the rule engine runs right after
the composition, before `det`; `PolicyModel.summaries` is a temporary
`HashMap<String, FullRoutineSummary>` built by COPYING `caps.direct_full` and `caps.coverage`
for the primary routines only (decision: copies, not a borrowed-slices refactor of
`PolicyModel`; primary direct facts are small next to raw, and the copy dies with the rule
engine). Then `Raw` is released and `det` takes `caps`.

- [ ] **Step 1: Record the oracle fixture** on Task 2b's head: for every r0 dir and the
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

Spec §7, §8 with §14.9's deltas, A6.3, A6.4, A6.7, A6.17, A6.20, A6.22, E14; bounds B1′, B2′,
B3, B4, B5; F6, F9, F11, F13, F-W1 to F-W3.

**Files:** create `docs/measurements/2026-10-<dd>-issue-160-step6.md`. Code only if a
fallback fires (its own commit, tests, proofs, gate, `check-diff`).

- [ ] **Step 1: Binaries**: head from the final head in `/work/bench/160-6/final/head-wt`;
  parent = `/work/bench/160-6/parent6a2/` (Task 1′). Bench script from
  `/work/bench/160-6/bench-draft.sh` into `/work/bench/160-6/final/bench.sh`, updated: B1′/B2′
  with +8,000 ms and +3,145,728 KiB; B6/B7 for `h` (+60,000 ms, ≤ 12,582,912 KiB); B3/B4 plain
  no-regression; B5 with the Task 3 keep/revert rule. Every peak on `alsem` (mimalloc).
- [ ] **Step 2: Part 1.** Histogram SHA equal. SARIF `cmp`: default analyze both universes and
  scopes; `o` (d47/d49/d51) cross-app at `--scope primary` and `--scope all`, and
  `--single-app` (A6.22; any move → F-W3). Policy custom and bundled `cmp` (A6.7, only if Task
  3 stays). Single-app fingerprint `cmp` except 2a's DAG moves. Digest: `--single-app` JSON diff
  (the DAG rule alone) and cross-app JSON diff (cone graph + D6 + DAG), each moved field
  attributed. `state-limit-exceeded` count parent vs head. Census: `non_primary_members=0`,
  `digest.witness_dags`, §4.10's buckets. `PD_EDGE_UNMAPPED == 0` (F6).
- [ ] **Step 3: Triage** (`cdo-measure`, `triage-findings`): the digest coverage/effects
  deltas, the truncation and conditionality flips. No d47/d49/d51 triage (none may move).
- [ ] **Step 4: Parts 2 and 3** in the quiet slot: B1′, B2′, B6, B7, B3, B4, B5 (void checks first;
  one re-run on void; F9 on void twice; a digest miss → F-W1/F-W2 once, then **D**: revert Task 2b's
  commit, digest stays on the cut; an `o` miss → STOP and re-panel; B5 → F13, then keep/revert Task 3).
  Part 1 also times the fingerprint query on CDO single-app (reported).
- [ ] **Step 5: `cdo-gate`** on the final committed head (after any revert); exit 0.
- [ ] **Step 6: Numbers file** `docs/measurements/2026-10-<dd>-issue-160-step6.md`: Task 1's
  STOP reading, the prototype numbers (spec §14.3.4, §14.4, §14.6), Task 1′'s re-measure,
  Parts 1-3, the bounds, triage, fallbacks fired, Task 3's fate, `cdo-gate`. `check-diff`,
  ledger `## Gate results`.

---

## Acceptance map (spec §6, as amended by §14.9)

| Acceptance | Where |
|---|---|
| A6.1, A6.9 (digest half) | Task 2b (T1); the d47 half → #161 follow-up |
| A6.2 | Task 2b (T8 coverage half) |
| A6.3 (B1′, B2′, B6, B7) | Task 1′ (S1′, S2′, S3′), Task 4 Step 4 |
| A6.4 (B3, B4 no-regression) | Task 4 Step 4 |
| A6.5 | Task 2b (T5) |
| A6.6 | Task 3 (T8 break 1), Task 4 Step 2 (census on `d`, `h`, policy; `o` is the stated I5 exception) |
| A6.7, A6.12 | Task 3 + Task 4 (only if B5 holds) |
| A6.8 | Task 1 Step 10 |
| A6.10 | Task 1 (T2), Task 2b (T3 digest arm) |
| A6.11 | #161 follow-up |
| A6.13 | Task 2b (T9, T3 digest arm) |
| A6.14 | Task 2b (T4) |
| A6.15 | Task 2b Step 3 (cut builder kept), Task 3 |
| A6.16 | Task 2b (T7) |
| A6.17 | Task 2b (T6), Task 4 Step 2 |
| A6.18 | Task 2b (checkpoint) |
| A6.19 | Task 2a Step 3 (DAG), Task 2b Step 5 (cone graph, D6) |
| A6.20 | Task 4 Step 3 |
| A6.21 | Task 2a (W1-W10, W4b, W6b, O1), Task 2b (B1-B4) |
| A6.23 | Task 1′ (P-4, P-5), Task 2a Step 3, Task 2b Step 5 |
| A6.22 | Task 2a Step 4 (early), Task 4 Step 2 |
| E14 | Task 4 Step 5 |
| B5 (policy peak) | Task 4 Step 4; decides whether Task 3 stays |
| Review Focus 5 (ordering with no subject) | void (ordering has no witness scope) |
