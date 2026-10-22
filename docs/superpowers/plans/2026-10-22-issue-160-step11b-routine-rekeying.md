# #160 step 11b: routine identity corrections, `RoutineIx` re-keying, the one collided-id rule — plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development
> (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps
> use checkbox (`- [ ]`) syntax for tracking.

**Goal:** three stages, in this order:
1. **C** (the corrections): J2, J5 and the adapter's caller check read each row's true node
   (`RowNodes.node_of`).
2. **A** (the re-keying): `RoutineIx` is stored where the producers need it, the carrier is
   split, the rule-bearing producers emit per row, and one projection makes today's per-id
   inputs before the computations. This stage is byte-neutral.
3. **B** (the one rule): the projection becomes the full-value union for set-valued inputs of a
   collided id, and the LAST owning row for single-valued lookups.

Only C moves output, by exactly the 14 pinned cross-app CDO results (spec §3.4).

**Architecture** (spec §4):
- C edits `RowNodes::build` (`src/program/model/workspace.rs`), the key join's caller check
  (`src/program/model/program_calls.rs`) and `program_event_graph`
  (`src/program/model/events.rs`).
- A adds:
  - `src/program/model/ix.rs` (`RoutineIx`);
  - `src/program/model/projection.rs` (the transient `rows_of_id`, the role check and the compat
    functions);
  - `CallEdge.from_ix`;
  - the `PrimaryGraph` / `typed` split of `ExecRelation`;
  - per-row output at every class S, K and P producer of spec §3.2;
  - the `site_pick` accessor read by d37 and `compose_roles_only`;
  - the `SolveIx` rename;
  - `Ord` on every union payload type.
- B replaces the compat functions' bodies with the two rules (spec §4.6) and moves #17 to the
  all-raises proof.

**Tech stack:** Rust; the crate's census hooks; the fail-closed identity scripts
`/work/bench/160-11b/identity-run.sh` and `/work/bench/160-11b/idcheck.py` (spec §8);
`scripts/agentflow` (`run`, `check-diff`); `scripts/cdo-gate`.

**Spec:** `docs/superpowers/specs/2026-10-22-issue-160-step11b-routine-rekeying-design.md`
(converged at `27772b6d`, 28 register entries, all accepted; "spec §n" below). Epic:
`docs/superpowers/specs/2026-10-18-issue-160-analysis-session-design.md` §9 row 11b, Decisions
6, 25, 57, 58. Probes P1-P16: `.agent/issue-160/ledger.md`.

There are three tasks, one per stage, in spec §4.1's order. Each task is one commit, closed by
its own fail-closed identity run before the next task starts:
- **Task C**: committed and identity-checked on its own, and mergeable alone (spec D1, F4, F5).
- **Task A**: byte-identical against Task C's commit.
- **Task B**: byte-identical against Task A's commit, unless F5's bounded admission applies.

11b is non-speed (Decision 58): no bench and no wall or peak bound. The census is reported only.

## Global Constraints

- **Editing.** Use the Edit/Write tools only. No `sed -i`, `perl -pi`, heredoc or script
  rewrites, including discrimination breaks (break with Edit, revert with Edit). Before running
  a break, re-read the edited line to confirm the break applied.
- **Formatting.** `rustfmt <file>` per touched `.rs` file; never `cargo fmt`.
- **No silencing, no skipping.** No `#[allow(` and no `#[ignore` anywhere in the branch
  (`check-diff` forbids both). Dead code is deleted, not silenced.
- **Protected paths, never edited:** `.github/`, `scripts/`, `.claude/`, `CLAUDE.md`,
  `.gitignore`, `Cargo.toml` version fields, `tree-sitter-al`, `.agent/` (except the ledger rows
  and Timeline lines a task names).
- **Environment.**
  - `export TREE_SITTER_AL_PATH=/work/alsem/tree-sitter-al` in every shell.
  - CDO only through a per-command prefix, with the path read from the main checkout's config
    at run time: `CDO_WS=$(jq -er .cdo_ws /work/alsem/.agent/config.json) <command>`.
  - Never write the path into any file, log name or ledger row, and never put `export CDO_WS`
    in a shell profile.
  - Never quote a timing from the `cdo` profile.
- **Per-task gate, before the commit.**
  1. `scripts/ci-steps task-affected > <log> 2>&1`, then `grep -nE 'FAILED|^error|test result'`
     on the log (never `| tail`; a pipeline hides the exit code).
  2. `scripts/check-goldens > <log> 2>&1`, with its exit status recorded.
  3. The pre-commit golden hook must pass unmodified (never `--no-verify`).
- **After each commit: `check-diff` on the new HEAD** (the 10+11a lesson: run before the commit,
  `--head HEAD` is still the previous commit):
  `python scripts/agentflow check-diff --base master --head HEAD --issue 160 --cwd /work/al-sem-issue-160-a5`.
  Required: `reasons: []`. Anything else is fixed forward in a new commit, never amended.
- **Staging.** Stage only the task's files (`git add <paths>`), never `git add -A`.
- **Discrimination proof** for every test a task turns green or changes:
  1. Break the code with Edit.
  2. Run the one test and record that it fails AT the assertion the spec names (the failing
     message, not a compile error or another test).
  3. Revert with Edit, run, record the pass.

  Record one ledger row per proof under `## Discrimination proofs`:
  `test | mutation | fail output | pass output | commit`. A break that comes back green is
  evidence about the test (CLAUDE.md): stop and report it, unless the spec names it as a stated
  LIMIT.
- **The identity runs are fail-closed and use the slot.** Each stage's run uses
  `identity-run.sh` with the expectation file the task names, through
  `python scripts/agentflow run --name bench --timeout 120 -- bash …`, with nobody building or
  editing in the worktree meanwhile.
  - The two binaries come from CLEAN detached worktrees of the two committed SHAs, never from
    the working tree.
  - `accept`'s summary (`summary.json`) and the run log are kept under
    `/work/bench/160-11b/idrun-<stage>/`.
  - The run's verdict goes to the ledger's `## Gate results`: the SHAs, the exit status, the
    violation count, and the `[retained-census] collision` lines.
  - **The next task does not start until that record exists and is green.**
- **A licensing `cdo-gate` run** is made on a COMMITTED head with nobody editing the tree:
  `git -C /work/al-sem-issue-160-a5 status --porcelain -- src tests crates` must be empty
  before and after. Command:

  ```
  CDO_WS=$(jq -er .cdo_ws /work/alsem/.agent/config.json) python scripts/agentflow run --name cdo-gate --timeout 60 --cwd /work/al-sem-issue-160-a5 -- bash scripts/cdo-gate
  ```

  The env prefix exports `CDO_WS` to the whole command tree, so it is not passed as an argument.
  That keeps post-merge's identity path. Any later code change re-runs it.

### The acceptance tests and the held-back tests

A separate agent is writing the acceptance tests in this worktree now (step 6). `check-diff`
forbids `#[ignore`, so no test of a later stage may sit in the branch. Before Task C's commit:
1. Every test named under **Task A** or **Task B** below, with its fixture files, is MOVED
   verbatim out of the worktree to `/work/bench/160-11b/held-tests/<task>/`. One file per test
   file, plus a `MANIFEST.md` listing each test name, its target file and its insertion point.
   The worktree must still compile without them.
2. Each later task restores its held tests verbatim, in the same commit as the code that turns
   them green.
3. A test's body changes only if it contradicts the spec. The change is recorded in the ledger
   with the spec clause.

Names below are the spec's. Where the spec gives no name, the plan gives one in **bold**; if
the step-6 agent used another, its name wins and the ledger maps the two.

## Review Focus

1. **C changes three reads and nothing else** (spec §4.2). `j2_row` differs from today's only
   on alias nodes. The caller check differs only on the 5 `GetPurchaseHeaderPageID` sites. J5
   differs only where `j5_compat_rows` counted (0 on CDO).
2. **A is byte-neutral by construction** (spec §4.5). Every compat function reproduces its
   point's CURRENT rule, BEFORE the computation. The cone, the solve, the event graph and the
   spans stay per string id. An id with one row is passed through by move.
3. **B touches only collided ids** (spec §4.6). Single-row ids are untouched. The union's
   identity is the whole payload and its order is the payload type's `Ord`, never a
   serialization. A single-valued lookup takes the LAST owning row per key, with every piece of
   one key from one row. Parameter roles and dispatch are class P. Guard frames (G) and the
   class R detectors are unchanged.
4. **No per-row cone, no twin-node fan-out, no `Role`** (spec §2, §12).

## Task C — the corrections (mergeable alone)

Spec §4.2, §5.4, §5.5, §5.8, §3.4, §8 (C), 11B-1, 11B-2, 11B-7. Depends on the step-6 tests
being in the worktree.

**Files:**
- `src/program/model/workspace.rs`, `RowNodes::build`:
  - the anchor map keeps every non-symbol-only row at an anchor, in row order;
  - the J2 loop picks the first with `node_of == n`; absent if none;
  - delete `decl_anchor` and its build loop (`:2193-2210`), `j5_node` and J5's span map
    (`:2226-2267`), and the `edges` / `demand` parameters;
  - `collision_census` computes `j2_alias_nodes` / `j5_compat_rows` against the corrected
    answers.
- `src/program/model/program_calls.rs`:
  - the caller check becomes `row_nodes.node_of[ri].as_ref() != Some(&ce.edge.from)`;
  - the two `RowNodes::build` callers lose the deleted arguments;
  - the new `#[doc(hidden)]` entry `cross_app_event_graph_after(inputs, model_instance_id,
    edit: impl FnOnce(&mut ModelRows))` on `cross_app_assemble`;
  - the module doc's "Caller check" line.
- `src/program/model/events.rs`: subscriber rows read `node_of[ri]`; the `None` arm stays
  (D7).
- Kept, unedited: `census::Key` / `anchor_key` (`census.rs`) and `join_reads`.
- Tests (as the step-6 agent wrote them; Task C's set):
  - `src/program/model/program_calls.rs` tests;
  - `tests/r4/r4_cross_app_program.rs`, including the T8a fixture with `OnB`;
  - `tests/program_resolve_harness.rs` (`adapter_cross_app_losses_on_cdo`).

**Acceptance tests turned green (or kept green) here:**
- T3:
  - `j2_row_one_anchor_two_nodes_is_each_nodes_own_row` (renamed from `…_is_the_first_row`);
  - `j2_row_two_anchors_one_node_is_the_last_arm` (the first arm's site now `matched`);
  - `j2_row_one_anchor_one_node_is_the_first_row` (unchanged).
- T4:
  - `caller_check_reads_node_of` (replaces `caller_check_reads_captured_decl_anchor`);
  - `caller_anchor_disagreement_is_caller_mismatch`, restated: a moved anchor alone no longer
    fails the check.
- T5: `event_graph_reads_node_of` (replaces `j5_node_keeps_the_dependency_span_map_answers`, on
  the `OnA`/`OnB` fixture).
- A8: `a_dependencys_if_duplicated_subscriber_gets_each_edge_once`, both cases; the second goes
  through `cross_app_event_graph_after`.
- T8:
  - `collision_census_counts_groups` (unchanged);
  - `adapter_cross_app_losses_on_cdo`, whose expected set is R-RESIDUAL minus the 5 (18
    `shape_mismatch`, 18 `adapter_l3_fallback_sites`). CDO; it runs in the licensing
    `cdo-gate`.
- `j5_compat_rows_counts_the_subscriber_gaps_only`:
  - after C the counter compares against `node_of`, so its gap fixture reads 0;
  - Task C changes only its expected count to 0, recording spec §4.2 ("The census") in the
    ledger; Task A deletes it with the counter;
  - if the step-6 agent already restated it, keep theirs.
- Kept green, unchanged: `collision_rules_hand_stated` (today's R1-R3 rules), every golden.

**Held back** (move to `held-tests/A/` and `held-tests/B/` before this commit; see Tasks A and
B): every test of spec §5.0's operation table except the ones above.

- [ ] **Step 1: The three reads** (spec §4.2). `rustfmt` the three files.
- [ ] **Step 2: The local CDO check** (correctness only):

  ```
  CDO_WS=$(jq -er .cdo_ws /work/alsem/.agent/config.json) cargo test --profile cdo --test program_resolve_harness adapter_cross_app_losses_on_cdo -- --nocapture > <log> 2>&1
  ```

  Required: green. Then one `ALSEM_RETAINED_CENSUS=1` cross-app analyze of the CDO workspace
  with a `cdo`-profile binary must print `j2_alias_nodes=0 j5_compat_rows=0`.
- [ ] **Step 3: Proofs** (spec §5.4, §5.5, §5.8):
  - T3: drop the node filter in the J2 loop → `…_is_each_nodes_own_row` fails (node B maps to
    row A).
  - T3: restore the anchor comparison in the caller check →
    `j2_row_two_anchors_one_node_is_the_last_arm` fails (`caller_mismatch`).
  - T4: make the caller check always pass → `caller_check_reads_node_of` fails.
  - T5: map a subscriber row to the first node at its span (the old rule) →
    `event_graph_reads_node_of` fails (row B emits `OnA`).
  - A8: break 8 (mark the id processed before the row maps, `events.rs` `processed`) → the
    second case fails.
  - T8: restore the anchor check → `adapter_cross_app_losses_on_cdo` fails, with the 5 back.
    CDO; run it with the prefix of Step 2.
- [ ] **Step 4: Gate and commit.**
  - Move the held tests out (see above) and confirm `cargo test --no-run` compiles.
  - Run the per-task gate.
  - Stage the three source files and Task C's test files.
  - Commit `#160 step 11b Task C: J2, J5 and the caller check read node_of`.
  - Run `check-diff` on the new HEAD.
  - Add a ledger Timeline line with the SHA and the proof rows.
- [ ] **Step 5: C's identity run** (spec §8, C). Build `alsem`/`aldump` (`cdo` profile) from
  clean detached worktrees of the branch base `5c42b8f3` (left) and Task C's SHA (right), then:

  ```
  python scripts/agentflow run --name bench --timeout 120 -- bash /work/bench/160-11b/identity-run.sh <L>/alsem <L>/aldump <R>/alsem <R>/aldump /work/bench/160-11b/idrun-taskC /work/bench/160-11b/expect-C-exact.json
  ```

  Required:
  - exit 0, 3,952 runs, 1,976 reports, 0 violations;
  - the 4 cross-app CDO reports differ in exactly the 14 pinned identities (rule + fingerprint
    + location), each message-only, "publishes N event(s)" +1;
  - every other report identical; the histogram identical;
  - the census line `j2_alias_nodes=0 j5_compat_rows=0`.

  Record the verdict under `## Gate results`.
- [ ] **Step 6: C's licensing `cdo-gate`** on Task C's committed head (Global Constraints
  command). Required: exit 0. C may merge alone (F4, F5 stop at it), so its head carries its
  own ratchet record. Record it under `## Gate results`.

**Fallbacks that can fire here** (spec §10):
- **F2**: `accept` reports any violation, or the census is not 0/0, or the loss gate is not the
  residual minus 5. This is a defect: diff the implementation against `probe-final.patch`'s
  rules and fix. Not closed in one attempt → stop at the base: nothing merges, and the ledger
  records why.
- **F3**: the base moved since the pin (the XmlPort fix, for example).
  1. Re-apply the prototype to the new base and repeat the identity run.
  2. Re-pin `expect-C-exact.json` only if every moved identity passes F3's source-based test
     (`d9`, message-only, +1, span root reaching `PageManagement.GetPurchaseHeaderPageID`).
  3. Anything else → stop, and re-panel C.

## Task A — the re-keying, the carrier split, the compat projection (byte-neutral)

Spec §4.3, §4.4, §4.5, §4.6 (only the `Ord` derives and `site_pick` in compat form), §4.8,
§4.9 (A's list), §5.0, §5.3 (T9), §5.7, 11B-3, 11B-6, 11B-8. Depends on Task C's identity run
and `cdo-gate` records.

**Before any code, re-read the merge points** of spec §3.2 classes S, K and P at Task C's SHA.
Write the per-point checklist into the ledger: file:line, today's rule, and the compat function
that reproduces it. Every row of §3.2's S, K and P cells appears once. A point found that §3.2
does not list fires **F6**.

**Files:**
- New `src/program/model/ix.rs`: `RoutineIx(pub u32)`, deriving `Copy, Clone, Debug, PartialEq,
  Eq, PartialOrd, Ord, Hash`.
- New `src/program/model/projection.rs`:
  - the transient `rows_of_id` (`HashMap<&str, SmallVec<[RoutineIx; 1]>>`, built from the
    current `ws.routines`, dropped after);
  - the role check: `debug_assert!` "collision group mixes roles", plus the census counter
    `groups_mixed_role`;
  - one compat function per rule, each with a doc line naming its point(s);
  - `site_pick` (compat form: the row's own `cs`, the first unioned edge, the LAST row's
    bindings).
- `src/program/model/calls.rs`: `CallEdge.from_ix`, set in `CallEdge::base`'s four callers
  (`program_calls.rs:999, :1638, :2155, :2259`).
- `src/engine/l4/exec_relation.rs`, `combined_graph.rs`:
  - `PrimaryGraph` (today's field names, no `typed_edges`) and `typed: Vec<TypedEdge>`, moved
    out in `ExecRelation::new`;
  - `primary()` returns `&Arc<PrimaryGraph>`;
  - every relation-fed `graph.typed_edges` reader moves to `rel.cone()`: d43, `digest.rs`,
    `snapshot.rs`, `snapshot_full.rs`, `ordering_engine.rs`, `ordering_inter.rs`,
    `unresolved_cone.rs`, `cone_derived.rs`, `capability_cone.rs`. Re-list them from
    `rg -n '\.typed_edges\b' src`; leave readers of another struct's field of that name alone.
- The producers of spec §3.2 classes S, K and P become per-row output, then the projection,
  then today's per-id input. Each point and its compat rule:

  | Points | Compat rule |
  |---|---|
  | `det_cone_inputs`; the cross-app base and r3a3 builders in `capability_cone.rs`; `summary_runner.rs`'s `base_summaries`, `dispatch_by_id` and lookups | LAST |
  | `events.rs` symbols | per-row duplicates kept |
  | `events.rs` `processed` | today's rule |
  | `combined_graph.rs` R1b | FIRST |
  | `combined_graph.rs` R1c, R1d | today's rule |
  | `transaction_spans.rs` #14, #16 | today's rule |
  | `event_param_temp.rs` | today's LAST-row raise read |
  | `detector_context.rs` R9 | FIRST |
  | `detector_context.rs` lookups (`routine_by_id`, `call_site_by_id`, `access_modifiers`, `root_classifications_by_routine`) | LAST |
  | `root_classification.rs` | today's rules |
  | `policy_engine.rs` | per row |
  | `return_summary.rs`, `ordering_facts.rs` | LAST |
  | snapshot anchors / identities / call-site evidence | today's rules |
  | `dep_artifact_l4.rs` | frames LAST, ops FIRST |
  | `unresolved_cone.rs` lookup | LAST |
  | `effect_query_cli.rs` `OperationJoin` | FIRST |
  | `d1_graph.rs` seeds | per-row copies |

  Classes U, G, F, R and "kept" are not touched.
- `d37.rs:224-245` and `compose_roles_only` (`summary_runner.rs:790-860`) read `site_pick`
  (compat form).
- `Converter::routine_by_id` (`program_calls.rs:1336`) is deleted; its reads use the
  projection's class P pick, LAST (= today). `AbiRowIds` is unchanged.
- The `SolveIx` rename: `src/engine/l4/routine_interner.rs` and its users. Re-list with
  `rg -lw RoutineIx src/engine`; 12 files at the spec's base.
- `PartialOrd, Ord` on every union payload type of spec §4.6: `CombinedEdge`, `TypedEdge`,
  `UncertaintyEdge`, `Uncertainty`, `DbEffect`, `CapabilityFact`, `RootClassification`,
  `SnapshotCallsiteEvidence`; plus `PartialEq, Eq, PartialOrd, Ord` on `EventSymbol`,
  `EventEdge`. A nested type that cannot derive `Ord` is given an explicit comparator over every
  field.
- `CollisionCensus`: delete `j2_alias_nodes`, `j5_compat_rows`; add `groups_mixed_role`.
- Test helper `force_shared_id_with_sites` (`detector_context.rs` tests), beside
  `force_shared_id`, which is unchanged.

**Acceptance tests restored from `held-tests/A/` and turned green here:**
- the total-order test, **`union_payload_types_have_a_total_order`** (`--lib`; calls
  `fn total<T: Ord>()` per payload type);
- T9 `collided_rows_share_one_role`:
  - `#[cfg_attr(debug_assertions, should_panic(expected = "collision group mixes roles"))]`;
  - release: `groups_mixed_role == 1`;
- T7 **`from_order_projects_a_collided_caller_once`**;
- the TODAY-form of each B test, which pins A's compat rule on a hand-stated input (spec §5.3,
  "through C and A: today's behaviour"):
  - T1 `collision_rules_hand_stated` (the rebuilt `#if` fixture, today's assertions);
  - **`collision_coverage_today`** (T1-cov);
  - T1b `collided_publisher_symbols_merge_by_value`;
  - T1c `collided_interface_call_counts_each_implementer_once`;
  - T1d `collided_id_is_one_reverse_entry`;
  - T1e `collided_commit_seeds_from_both_arms`;
  - T1f `event_param_temp_proves_over_every_raise`;
  - T1f-neg `event_param_temp_mixed_raise_is_not_proven`;
  - T1g `dep_order_index_pairs_frames_and_ops_of_one_row`;
  - T1h `collided_routine_classifies_once`;
  - T1i `policy_evaluates_a_collided_routine_once`;
  - T1j `collided_parameter_role_is_one_record`;
  - T6 **`collided_call_site_pieces_come_from_one_row`**;
  - T6b **`per_site_consumers_read_one_row`**;
  - T-guard **`collided_guard_frames_are_unchanged`**.

  If the step-6 agent wrote only the B-form, the implementer derives the today-form from the
  spec's "today" column. That is the same fixture with today's expected values, under the same
  test name. The derivation is recorded in the ledger; the B-form stays held in
  `held-tests/B/` verbatim.

**Deleted here:** `j5_compat_rows_counts_the_subscriber_gaps_only` (with the counter). The
interner's tests are renamed (`SolveIx`).

**Held for Task B:** T2 **`collided_set_valued_surfaces_ignore_row_order`**, T2u
**`union_orders_equal_key_items_by_full_value`**, and the B-form of every test listed above.

- [ ] **Step 1: `RoutineIx`, `CallEdge.from_ix`, `SolveIx`, `Ord` derives**, plus the
  total-order test. The pre-commit golden check must show `cli-query-goldens` unchanged.
- [ ] **Step 2: The carrier split.** `cargo test --test cli` and `--test r3` green (r3a1 and the
  snapshot still read the same bytes).
- [ ] **Step 3: `projection.rs` and the per-row producers**, point by point from the ledger
  checklist. After each group of points, run the module's `--lib` tests and
  `scripts/check-goldens`.
- [ ] **Step 4: `site_pick` (compat) in d37 and `compose_roles_only`; the role check and
  counter.**
- [ ] **Step 5: Restore `held-tests/A/`** and confirm every test above is green.
- [ ] **Step 6: Proofs** (spec §5.0):
  - Compat projection: in one compat function, swap LAST ↔ FIRST (for R2a, `det_cone_inputs`)
    → `collision_rules_hand_stated`'s today-form R2 assertion fails. Repeat for #17 (read the
    FIRST row's raise) → T1f-neg's today-form (proven) assertion fails.
  - Total order: add a `HashMap` field to one payload type → the build fails at
    `union_payload_types_have_a_total_order`. Record the compile error as the "fail output":
    the spec names the build as the check.
  - T9: remove the projection's call to the role check → no panic in debug → fails.
  - T7: drop the dedup in the `from_order` projection → the id appears twice → fails.
  - `site_pick` compat: read the LAST row's edge instead of the first unioned one → T6b's
    today-form fails.
- [ ] **Step 7: Gate and commit.**
  - Run the per-task gate.
  - `rg -n 'j5_compat_rows|j2_alias_nodes|routine_by_id: HashMap' src` hits nothing but the
    deleted-name history, which is none.
  - Commit `#160 step 11b Task A: RoutineIx, the carrier split, per-row producers and the compat projection`.
  - Run `check-diff` on the new HEAD.
  - Add a ledger Timeline line and the proof rows.
- [ ] **Step 8: A's identity run.** Left = Task C's SHA, right = Task A's SHA,
  `/work/bench/160-11b/idrun-taskA`, expectation `/work/bench/160-11b/expect-identical.json`.

  Required:
  - exit 0, 0 violations;
  - the histogram identical;
  - the census reported, with `groups_mixed_role=0` in both universes.

  Record it under `## Gate results`.

**Fallbacks that can fire here:**
- **F4** (A's run reports a violation, a golden or a `--lib` outcome outside spec §5.9 moves):
  a compat function is not today's rule. Bisect Step 3's point groups, fix that function, and
  re-run. Not closed in one attempt → stop at Task C's head (C may merge; A does not).
- **F6** (a merge point §3.2 does not list): it joins union, LAST owning, or "kept" with its
  reason, recorded in spec §3.2 and the ledger. If it needs a third rule → stop, and re-panel.
- **F7** (a census bucket grows more than 20 %): name it and make the projection consume the
  per-row output (spec §4.8); re-run the census once. Still more than 20 % → report it to the
  final panel; this does not block.
- **F8** (more than 40 reader files for one point): keep the producer per-row only; the readers
  keep the projected value.

## Task B — the one rule (and the claim's closing docs)

Spec §4.6, §4.7, §5.1, §5.2, §5.3, §5.6, §8 (B), §9, §12, 11B-4. Depends on Task A's identity
run record.

**Files:**
- `src/program/model/projection.rs`: the compat functions' bodies become the two rules:
  - Set-valued (classes S, K, and class U's collided lists):
    - the full-value union across ONE collided id's rows, with identity = the whole payload;
    - sorted by today's sort key, then by `Ord`;
    - single-row ids untouched;
    - flags OR-ed;
    - coverage takes the least complete status (`complete` < `partial` < `unknown`), and reasons
      a sorted union.
  - Single-valued (class P): the LAST OWNING row per key, for call sites, ops and parameter
    indexes. Every piece of one key comes from that one row (edge + bindings; frames + ops;
    anchors). R9's within-row first implementer is kept.
- `site_pick` (consistent form): d37 and `compose_roles_only` iterate the id's owned call-site
  keys.
- `src/program/model/event_param_temp.rs`:
  - each raise is read at `(RoutineIx, site)` via `CallEdge.from_ix`;
  - proven only if EVERY applicable raise proves temp;
  - the rewrite applies to every subscriber row of the id.
- `src/engine/root_classification.rs`: `resolve_target` dedups its matches by id first.
- `src/engine/l5/detector_context.rs`: delete the R3a comment block (`:1412-1468`) and the
  `debug_assert` on how cones and `direct_full` collide (I2's exception, spec §5.9).
- `CHANGELOG.md`, under `[Unreleased]` → `### Changed`, "#160 step 11b" (Part of #160, no
  closing keyword):
  - the corrections and their measured move;
  - `RoutineIx` / `SolveIx`, the carrier split, the projection, the one rule;
  - classes G and R unchanged;
  - amendments 11B-1 to 11B-8;
  - no golden moved.
- `docs/measurements/2026-10-22-issue-160-step11b.md` (new):
  - the three identity runs' verdicts (SHAs, run and report counts, violations);
  - the C move (14 identities per report, by span root);
  - the census lines per stage and both universes;
  - the cdo-gate records;
  - "no bench (Decision 58)".

**Acceptance tests restored from `held-tests/B/` (replacing their today-forms) and turned
green here:**
- T1 `collision_rules_hand_stated`:
  - direct facts A ∪ B;
  - X and Z inherited;
  - `db_effects` from both arms.
- **`collision_coverage_today`** → B-form (least complete; sorted reason union, through
  `build_detector_context` and the snapshot's `coverage`).
- T2 **`collided_set_valued_surfaces_ignore_row_order`**:
  - compares the set-valued surfaces of spec §5.2, normalized;
  - the event graph is excluded (R3-S1);
  - classes P, R and G are excluded by design.
- T2u **`union_orders_equal_key_items_by_full_value`**:
  - the projection-helper union keeps both conflicting items, ordered by `Ord`, equal in both
    input orders;
  - production `edges_by_from[id]` is equal in both row orders, holds both edges, and is ordered
    by `Ord`;
  - **production `uncertainty_edges_by_from[id]` (Opus round-4 nit, R3-S2)**: in EACH row order
    it holds exactly ONE interface uncertainty for the call-site key, the one of THAT order's
    LAST owning row. The test asserts it per order against that order's last owning row's
    `interface_name`. It does NOT assert that the two orders are equal: they keep different
    rows' `interface_name` by design.
- T1b-T1j (B-forms), T6 (B-form: `cs4` from arm A, `cs0` edge and bindings from arm B and from
  one row, `cs1` first implementer), T6b (B-form, including the single-row multi-implementer
  case equal to today), T-guard (unchanged frames; its today-form and B-form are the same).
- `hand_stated_id_collision_keeps_a_real_summary_and_derived_row` and
  `colliding_ids_keep_the_full_summary_union_not_just_the_edges`, restated to the union on their
  fixtures.

- [ ] **Step 1: The two rules** in `projection.rs`, then `site_pick` consistent, then #17, then
  `resolve_target`.
- [ ] **Step 2: Restore `held-tests/B/`**; every test above green; `held-tests/` has nothing
  left unrestored (`MANIFEST.md` ticked).
- [ ] **Step 3: Proofs** (spec §5.0, §5.1, §5.2, §5.3, §5.6):
  - T1: projection first-row → "B's modify in the direct facts" fails; last-row → "A's insert"
    fails.
  - T1-cov: LAST status → the `partial` case reads `complete`; LAST reasons → one reason
    missing.
  - T2: revert one union to a first-row pick, for R2a, #13 and the snapshot's K dedup in turn
    → that surface differs → fails.
  - T2u:
    - dedup by sort key only → one item lost;
    - emission order instead of `Ord` → the two orders differ (edges);
    - for the uncertainty half: take the FIRST owning row → each order's assertion fails.
  - T1b: push a symbol per row → two dispatch edges.
  - T1c: union without dedup → `candidate_count` doubled.
  - T1d: walk duplicate `nodes` → the edge is listed twice.
  - T1e: seed `routine_by_id[id]` → the wrong seed row.
  - T1f / T1f-neg: inspect raises through the LAST-row string index → T1f-neg proves (wrongly)
    → fails.
  - T1g: today's pairing → frames and ops from different rows.
  - T1h: keep per-row copies → two classifications; count rows → "ambiguous".
  - T1i: evaluate per row → counters doubled.
  - T1j: union the role records → two records for index 1 → fails.
  - T6:
    - `rows_of_id.last()` without the owning filter → `cs4` loses its edge;
    - edge FIRST and bindings LAST → the same-row assertion fails;
    - last implementer → `cs1` fails.
  - T6b: read `edges_by_from` directly → fails; iterate one row's sites → `cs4` missing.
  - T-guard: union the arms' requirements → fails.
- [ ] **Step 4: Gate.**
  - Run the per-task gate.
  - `scripts/ci-steps clippy > <log> 2>&1` (release, `-D warnings`; no compat function left
    unused).
  - `rg -n 'compat' src/program/model/projection.rs` shows only doc text that names the
    historical rule, if any.
- [ ] **Step 5: CHANGELOG and the numbers file**:
  - the C and A rows filled from the ledger's `## Gate results`;
  - B's row `<pending B identity run>`, filled in a docs-only commit after Step 7.
- [ ] **Step 6: Commit and check.**
  - Commit `#160 step 11b Task B: the one rule (full-value union, last owning row)`.
  - Run `check-diff` on the new HEAD.
  - Add a ledger Timeline line and the proof rows.
- [ ] **Step 7: B's identity run.** Left = Task A's SHA, right = Task B's SHA,
  `/work/bench/160-11b/idrun-taskB`, expectation `expect-identical.json`. Required: exit 0 and
  0 violations, or F5's bounded admission (`expect-B.json` written from the admitted list).
  Record it.
- [ ] **Step 8: The branch's licensing `cdo-gate`** on the final committed head (Global
  Constraints command). Required: exit 0. Then the docs-only commit filling the numbers file's
  B row (`#160 step 11b: identity-run numbers`), and `check-diff` on it.

**Fallbacks that can fire here:**
- **F5** (B's run reports a violation): triage each moved result to a §3.2 point and a collided
  id.
  - **Admissible**: exactly the union or the LAST owning pick applied to that id's rows as read
    from the AL source, or its propagation into a caller's result.
  - Triage tools: `golden-diff-triager` for goldens; `cdo-measure` + `triage-findings` for CDO.
  - Each admitted move is listed in the numbers file with its source reading, and
    `expect-B.json` is written from that list.
  - ONE fix attempt; not closed → stop at Task A's head (C and A may merge).
- **F11** (T6b's single-row case or B's run shows `site_pick` changing a SINGLE-row answer):
  - the accessor keeps today's edge choice for single-row ids;
  - the exception is stated in the spec (§4.6), with a test pinning that shape;
  - B's run is repeated once; still not identical → F5.
- **F6** and **F7** as in Task A.
- **F10** (measured: true; B has no CDO witness): stated in the numbers file as a limit.

---

## Acceptance map (spec §6)

| Acceptance | Where |
|---|---|
| A11b.1, A11b.2, A11b.3 | Task C (T3, T4, T5, A8, T8; the census line) |
| A11b.4 | Task C Step 5 (`expect-C-exact.json`, 0 violations) |
| A11b.5, A11b.6, A11b.7 | Task A (`RoutineIx`, `from_ix`, `SolveIx`, `PrimaryGraph`, the per-point checklist) |
| A11b.8 | Task A Step 8 (`expect-identical.json` against C) |
| A11b.9 | Task A (total order, T9, T7, today-forms), Task B (T1, T1-cov, T2, T2u, T1b-T1j, T6, T6b, T-guard) |
| A11b.10 | Task B Step 7 |
| A11b.11 | every task (`scripts/check-goldens`; the P15 id-pattern script re-run on Task B's head equals `/work/bench/160-11b/p15-ids-base.tsv`) |
| A11b.12 | the census line of each identity run |
| A11b.13 | no bench (Decision 58); `cdo-gate` records of Task C Step 6 and Task B Step 8 |
| E14 | Task C Step 6, Task B Step 8 |
