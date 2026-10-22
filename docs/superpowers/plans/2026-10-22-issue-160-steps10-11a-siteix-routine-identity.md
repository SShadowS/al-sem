# #160 steps 10 + 11a: `SiteIx` (the key join) and routine identity — plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development
> (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps
> use checkbox (`- [ ]`) syntax for tracking.

**Goal:** the model and the program engine name a call site by one key `(FileIx, ExprId)`;
the adapter pairs by that key; the span join, `texts` and J4 are deleted (step 10). At
assembly, each row's node, each read node's program declaration anchor, and today's J2 and J5
answers are recorded once (`RowNodes`); the caller check, J2 and J5 read that table (step
11a). No output byte moves.

**Architecture** (spec §4):
- `src/program/site_key.rs`: `FileIx`, `SiteIx`, `ProgramSite`, `SiteExprs`, `FileTable`
  (the one file table, on `ProgramContext`).
- The resolver stamps `ClassifiedEdge.site` (key + `fires`); the walk returns `SiteExprs`
  beside `PFeatures`; `project_ir` stores `file` + `site_exprs` on each `ModelRoutine`. The
  LSP path passes `None` (§4.2a).
- `RowNodes` (`node_of`, `decl_anchor`, `j2_row`, `j5_node`) is built at the end of assembly
  and travels in `ModelRows` to the adapter and the event graph; it is not retained.
- Task 1 stamps keys and builds the tables while production still reads the old joins; the
  test-only oracles (T1, T7, T8) compare old and new. Task 2 switches production to the key
  join and the recorded answers. Task 3, the LAST code commit, deletes the old joins and the
  oracles. Each switch and the deletion are licensed by a recorded `cdo-gate` run on the
  commit before them (spec §4.12).

**Tech stack:** Rust; the crate's census hooks (`census.program`, `census_model_rows`);
`scripts/agentflow` (`cdo-gate`); `scripts/bench_ab.py` and `/work/bench/160-10-11a/maxrss.py`
(conductor only).

**Spec:** `docs/superpowers/specs/2026-10-22-issue-160-steps10-11a-siteix-routine-identity-design.md`
(converged at `f52ac8be`, 29 register entries all accepted; "spec §n" below). Epic:
`docs/superpowers/specs/2026-10-18-issue-160-analysis-session-design.md` §9 rows 10, 11a,
Decisions 57, 58. Probes P1-P15: `.agent/issue-160/ledger.md`.

There are three tasks, each one commit, in spec §4.12's order. The two licensing
`cdo-gate` runs are the first step of Task 2 and of Task 3: they commit nothing, so no extra
task is needed (7b had a separate Task 3 only because its licensing run followed a docs
commit). The CDO bench is run by the conductor after the final panel, not in a task. The
claim is byte-neutral: **no golden may move in any commit** (spec E2, epic I2).

## Global Constraints

- **Editing.** Use the Edit/Write tools only. No `sed -i`, `perl -pi`, heredoc or script
  rewrites, including discrimination breaks (break with Edit, revert with Edit). Before
  running a break, re-read the edited line to confirm the break applied.
- **Formatting.** `rustfmt <file>` per touched `.rs` file; never `cargo fmt`.
- **No silencing.** No `#[allow(`. Dead code is deleted, not silenced.
- **Protected paths, never edited:** `.github/`, `scripts/`, `.claude/`, `CLAUDE.md`,
  `.gitignore`, `Cargo.toml` version fields, `tree-sitter-al`, `.agent/` (except the
  ledger rows and Timeline lines a task names).
- **Environment.** `export TREE_SITTER_AL_PATH=/work/alsem/tree-sitter-al`.
  - CDO only through a per-command prefix, with the path read from the main checkout's
    config: `CDO_WS=$(jq -r .cdo_ws /work/alsem/.agent/config.json) cargo test --profile cdo …`.
  - Never export `CDO_WS`, and never write its path into a committed file or the ledger.
  - Never quote a timing from the `cdo` profile.
- **Per-task gate, before the commit.**
  1. `scripts/ci-steps task-affected > <log> 2>&1`, then `grep` the log for `FAILED`,
     `error` and the summary line (never `| tail`).
  2. `cargo test -p al-sem --lib program_calls` and `cargo test -p al-sem --lib
     program::resolve::full` green (the two modules most of the change sits in).
  3. The pre-commit golden hook must pass unmodified (never `--no-verify`).
- **Staging.** Stage only the task's files (`git add <paths>`), never `git add -A`.
- **Discrimination proof** for every test a task turns green or changes:
  1. Break the code with Edit.
  2. Run the test and record that it fails AT the assertion the spec names (the failing
     message, not a compile error or another test).
  3. Revert with Edit, run, record the pass.
  One ledger row per proof under `## Discrimination proofs`:
  `test | break | fail output | pass output | commit`. The breaks are spec §5's, listed per
  task below. A break that comes back green is evidence about the test (CLAUDE.md): stop and
  report it, unless the spec names it as a stated LIMIT.
- **The acceptance tests come from a separate agent** (ledger `### Acceptance tests (step
  6)`). They are in the worktree, uncommitted, with behaviour-neutral stubs. They cannot be
  committed alone (the pre-commit hook runs `check-goldens`), so **Task 1's commit carries
  ALL of them with its implementation**. Some can only turn green when production switches
  (Task 2). Those are committed in Task 1 with
  `#[ignore = "#160 Task 2: needs the key join in production (spec §4.12)"]` and nothing
  else changed; Task 2 removes every such attribute, and Task 2's gate asserts
  `rg -n '#160 Task 2: needs' src tests` is empty. Do not rename a test. Change a test's body
  only if it contradicts the spec, and record that change in the ledger with the spec clause.
- **The test author's notes, carried in** (ledger, step 6):
  - `span_join_under_demand` must apply the §4.5 demand filter at Task 1 (it is unfiltered
    in the stub), or T1's cross-app census comparison is against the wrong baseline;
  - T1's CDO identity keys R-PAIRS by `(row name, csN, path, line, col)`, not the routine-id
    hash; keep that;
  - R-RESIDUAL hard-codes the Base Application id; keep it;
  - T8a asserts J5's "first node in graph routine order" for `Hb`: check it against the REAL
    `j5_node` once Task 1 builds it; if it disagrees, J5's own rule (spec §4.9) decides and
    the test is corrected with the clause recorded;
  - T4b's procedure is `Work` (not `Run`, a built-in).

## Review Focus

1. **Production reads nothing new in Task 1** (spec §4.12). The stamps, `site_exprs`,
   `RowNodes`, the census buckets and the oracles exist; `join`, `l3_routine`, the caller
   check and the event graph still run the old code. Bench-level identity of Task 1 is
   therefore trivial; its risk is cost (stamps in the resolver, tables in assembly).
2. **The oracles never run inside production code** (spec §4.12, R1-07): `key_join_parity`,
   `j2_differences` and the J5 comparison are called by tests only, on pristine assembly.
3. **`j2_row` is J2 exactly**, including the cross-node alias (spec §4.7): first
   non-symbol-only row over ALL rows at the surface's declaration anchor, on J2's own domain.
4. **`decl_anchor` is captured from the surface, never from a row** (spec §4.8); T9 and
   `caller_anchor_disagreement_is_caller_mismatch` pin both sides.
5. **`j5_node` keeps J5's two wrong answers** (spec §4.9) including `None`.
6. **The span debug assertion fires only on accepted pairs** (spec §4.3 step 4), so tests
   that unpair by key never reach it.
7. **No `ExprId` constructor outside `al-syntax`**: keys hold `ExprId::index()` as `u32`;
   unused keys in tests are `u32::MAX - e` (spec §4.1, §5.3).

## Task 1 — keys, tables, census buckets and the oracles; production unchanged

Spec §4.1, §4.2, §4.2a, §4.4 (stamping only), §4.6, §4.9 (the table only), §4.10, §4.13
(buckets), §5.1, §5.6-§5.11. Not small; one commit, because the acceptance tests are red
without the code.

**Files** (the stubs are already in the worktree; this task makes them real):
- `src/program/site_key.rs`: `FileTable` (real already), nothing else new;
- `src/program/resolve/full.rs`: `ProgramContext.files` built at construction (`:1861`);
  `resolve_file_obligations(.., file: Option<FileIx>)` stamps `ProgramSite` at the call-site
  push (`:1053`) when `Some`, with `fires = can_fire_trigger(op_lc)` for a `RecordOp`;
  `can_fire_trigger` moves here (re-exported where `program_calls.rs` reads it); the workspace
  (`:1245`) and dependency (`:1657`) callers pass `ctx.file_ix(app, path)`;
- `src/lsp/snapshot.rs`: `recompute_file` passes `None` (unchanged in the stub, confirm);
  the destructure gains `files: _`;
- `src/program/body/ir_walk.rs`: the walk records `eid.index() as u32` at the three pushes
  (`:997`, `:1045`, `:1086`); `project_routine_features_ir_keyed` returns
  `(PFeatures, SiteExprs)`; the two existing entries keep their signatures;
- `src/program/model/workspace.rs`: `project_ir` takes `file: Option<FileIx>` and stores
  `file` + `site_exprs` (with the three-length `debug_assert_eq!`); both callers look the
  file up next to `ParenlessCalls`; `RowNodes` built at the end of assembly (`node_of` from
  physical rows / `source_routine_node_id` / J3; `decl_anchor`, `j2_row`, `j5_node` by
  spec §4.6-§4.9) and returned in `ModelRows`;
- `src/program/model/program_calls.rs`: the real `key_join` (still unread by production),
  `span_join_under_demand` with the §4.5 filter, `key_join_parity`, `j2_differences`,
  `collision_census`; `cross_app_rows_before_attach` exposes `RowNodes` for A8;
- `src/program/model/events.rs`: `program_event_graph_with_row_nodes` reads `j5_node`
  (test-only caller until Task 2);
- the census buckets of spec §4.13: `program.edge_site`, `program.file_table`,
  `program.row_nodes` (in `census.program`), `model.site_exprs` (in `census_model_rows`);
- every acceptance-test file the step-6 agent touched (ledger list), the 12 `ModelRoutine`
  and 10 `ClassifiedEdge` test literals already updated by the stubs.

**Acceptance tests turned green here:**
- T1 `key_join_equals_span_join_on_the_fixtures`; `key_join_equals_span_join_on_cdo`
  (CDO, in the licensing run);
- T6 `record_op_site_fires_matches_can_fire_trigger`;
- T7 `j2_row_equals_decl_join`, `j2_row_equals_decl_join_on_cdo` (CDO);
- T8 `event_graph_j5_node_equals_span_join`, `event_graph_j5_node_equals_span_join_on_cdo`
  (CDO);
- T7a-c `j2_row_two_anchors_one_node_is_the_last_arm`,
  `j2_row_one_anchor_two_nodes_is_the_first_row`, `j2_row_one_anchor_one_node_is_the_first_row`;
- T8a `j5_node_keeps_the_dependency_span_map_answers`;
- A8 `a_dependencys_if_duplicated_subscriber_gets_each_edge_once` (its precondition reads
  `j5_node`);
- T9 `collision_census_counts_groups`;
- T10 `resolve_file_obligations_unstamped_and_stamped`,
  `resolve_file_obligations_matches_full_run_per_file_and_in_concatenation`;
- green by design, kept green: `collision_rules_hand_stated`, `duplicate_program_key_is_counted`,
  `duplicate_l3_span_is_counted`, `adapter_loses_nothing_cross_app_on_the_fixtures`.

**Committed with `#[ignore = "#160 Task 2: …"]`** (turn green in Task 2): T2
`cross_app_census_has_no_losses_with_a_dependency_body`, `adapter_cross_app_losses_on_cdo`;
T3 `l3_ops_with_no_program_site_are_counted_by_trigger_capability`,
`a_site_without_a_program_edge_is_unknown_not_legacy_resolved`,
`unmatched_record_op_gets_no_l3_trigger_edge`, `unmatched_site_is_unknown_not_l3`,
`interface_implementer_outside_l3_is_unknown`; T4 `duplicate_model_key_per_category`; T4b
`two_apps_same_path_same_expr_pair_within_their_app`; T5
`span_assertion_fires_on_a_key_pair_whose_spans_differ`; T9
`caller_check_reads_captured_decl_anchor`. (If one of these is in fact green at Task 1, do
not ignore it; record that in the ledger.)

- [ ] **Step 1: Stamp the program side** (`full.rs`, `snapshot.rs`). Run T6 and T10 → green.
- [ ] **Step 2: Stamp the model side** (`ir_walk.rs`, `workspace.rs`). Confirm the l2
  snapshot is unchanged: `cargo test --test l2_ir` green (spec §2: nothing new is
  `Debug`-visible on a feature type).
- [ ] **Step 3: `RowNodes`, `collision_census`, the event-graph variant.** T7a-c, T8a, A8,
  `collision_census_counts_groups` → green. Check T8a's `Hb` answer against the real
  `j5_node` (note above).
- [ ] **Step 4: The real `key_join`, `span_join_under_demand` (with the §4.5 filter) and the
  oracles.** T1, T7, T8 on the fixtures → green with 0 differences.
- [ ] **Step 5: Census buckets** (spec §4.13). One local `ALSEM_RETAINED_CENSUS=1` analyze of
  an r0 fixture shows the four new buckets non-zero (sanity only; CDO bytes are the bench's).
- [ ] **Step 6: CDO variants, local run** (correctness only):
  `CDO_WS=$(jq -r .cdo_ws /work/alsem/.agent/config.json) cargo test --profile cdo -p al-sem
  --lib -- key_join_equals_span_join_on_cdo j2_row_equals_decl_join_on_cdo
  event_graph_j5_node_equals_span_join_on_cdo --nocapture > <log> 2>&1`. Required: T1 equals
  R-PAIRS exactly with identical conversion and the census deltas of spec §5.1 point 3
  (`duplicate_program_span` 24 → 0, `duplicate_l3_span` 23 → 0, `operation_site` +1, cross-app
  under the filter); T7, T8 zero differences. (The licensing run of Task 2 Step 0 repeats this
  inside `cdo-gate`.)
- [ ] **Step 7: Proofs** (spec §5):
  - T1: swap the first two call sites' `site_exprs` entries of one fixture row → pairs
    differ → fails;
  - T6: `fires = true` always → the `SetRange` assertion fails;
  - T7: compute `j2_row` within `rows_of[node]` only (per node) → T7b's fixture differs → fails;
  - T8: leave dependency rows out of `j5_node` → a dependency subscriber's edge disappears → fails;
  - T7a/T7c: `j2_row` = last row → T7c fails; T7b: per-node selection → fails; T8a:
    `j5_node = node_of` → both J5 assertions fail;
  - A8: break 8 (mark an id processed before it maps, `events.rs` `processed`) → fails;
    and set `j5_node[first row] = node_of[first row]` → the precondition assertion fails;
  - `collision_census_counts_groups`: count distinct rows instead of ids → 0 → fails;
  - `collision_rules_hand_stated`: `direct_full.insert` → `entry().or_insert` at
    `detector_context.rs:1261` → the R2 assertion fails;
  - T10: stamp `Some(FileIx(0))` when `file` is `None` → the `None` assertion fails.
- [ ] **Step 8: Gate and commit.**
  - `scripts/ci-steps task-affected` (the ignored tests show as ignored, every other green);
  - `scripts/check-goldens` green (no golden moves);
  - stage the production files above and every acceptance-test file;
  - commit: `#160 steps 10+11a Task 1: stamp SiteIx keys and record RowNodes; old joins
    still in production, oracles compare`;
  - ledger Timeline line with the sha and the proof rows.

**Fallbacks that can fire here** (spec §10): **F1** (T1 on r0/fixtures differs) → Task 2 does
not start; one walk-alignment fix, else stop at Task 1. **F2** (T1 on CDO ≠ R-PAIRS, a
conversion differs, or the census deltas differ) → same as F1; R-PAIRS is never widened to
admit a differing conversion. **F5** (T7 ≠ J2) → make assembly read the adapter's surface and
rows, re-run T7; not closed → stop at Task 1. **F6** (T8 ≠ J5) → same rule for J5's maps;
not closed → stop at Task 1. **F4** (a golden or `--lib` outcome outside spec §5.12 moves) →
fix; not fixed in one attempt → blocked. "Stop at Task 1" means: Task 1's commit stays (keys
and tables, nothing switched), the ledger records why, and the conductor reports blocked.

## Task 2 — the switch

Spec §4.3, §4.5, §4.7, §4.8, §4.9, §5.2-§5.5, §5.9 (caller check). Depends on Task 1 and on
Step 0's green record.

**Files:** `src/program/model/program_calls.rs` (`join` → key join with the span debug
assertion; `l3_routine` → `j2_row`; caller check → `decl_anchor`; `attach_program_calls_with`
and `cross_app_assemble` pass `demand`; `adapter` reads `RowNodes`),
`src/program/model/events.rs` (`program_event_graph` reads `j5_node`), the acceptance-test
files (remove the `#160 Task 2` ignores only).

- [ ] **Step 0: The licensing run (no commit).** On Task 1's commit
  (`git status --porcelain -- src tests crates` empty), run
  `python scripts/agentflow run --name cdo-gate --timeout 45 --cwd /work/al-sem-issue-160-a4 -- bash scripts/cdo-gate`.
  Required: exit 0 and the T1/T7/T8 CDO lines as in Task 1 Step 6. Record under the ledger's
  `## Gate results`: the sha, the exit status, the log path, the three oracle lines. **Task 2
  is committed only after this record exists and is green.** A red run fires F2/F5/F6 (Task 1's
  rules), never a patch-around.
- [ ] **Step 1: The key join in production** with the span debug assertion on every accepted
  pair (spec §4.3 steps 1-6), the per-category duplicate accounting, `fires` for J4's
  classification, the §4.5 filter. Remove the ignores of T2, T3, T4, T4b, T5.
- [ ] **Step 2: `j2_row`, `decl_anchor`, `j5_node` in production.** Remove the ignore of
  `caller_check_reads_captured_decl_anchor`.
- [ ] **Step 3: Whole debug suite with the span assertion live, J1 still present** (A10.6):
  `scripts/ci-steps task > <log> 2>&1` (the full task gate, not `-affected`, once). Record
  the result in the ledger.
- [ ] **Step 4: CDO** (correctness): `adapter_cross_app_losses_on_cdo` and T1/T7/T8 CDO
  variants, with the prefix above. Required: the residual equals R-RESIDUAL exactly; T1, T7,
  T8 unchanged from Task 1.
- [ ] **Step 5: Proofs** (spec §5):
  - T2 (epic row 10): in the key join, replace `DepA`'s call-site key with `u32::MAX` →
    `no_program_site` 1 and `program_only_site` 1, asserted by name → fails;
  - T2 (10A-2): drop the demand filter → `DepUnused`'s edge is a `program_only_site` → fails;
  - T3: pair ops by `(ri, oi)` position instead of key → the first test's
    `l3_op_no_program_site_*` drop to 0 → fails;
  - T4: last-wins without counting → `duplicate_program_key_is_counted` fails; count across
    categories → `duplicate_model_key_per_category`'s second assertion fails;
  - T4b: key `FileTable` by path only → one app's site pairs with the other's edge
    (`caller_mismatch`) → fails;
  - T5: delete the span assertion → no panic → fails;
  - T9: compare the row's anchor with `j2_row`'s row anchor (a row-derived side) → the sites
    match → `caller_check_reads_captured_decl_anchor` fails.
- [ ] **Step 6: Gate and commit.**
  - `scripts/ci-steps task-affected`; `rg -n '#160 Task 2: needs' src tests` empty;
    `scripts/check-goldens` green;
  - commit: `#160 steps 10+11a Task 2: join by SiteIx; caller check, J2 and J5 read RowNodes`;
  - ledger Timeline line and proof rows.

**Fallbacks that can fire here:** **F7** (the CDO residual ≠ R-RESIDUAL) → T2 fails by
design; a key-join or filter defect → fix, re-run T1 and T2; the set is never widened; not
closed in one attempt → stop at Task 1 (revert Task 2's uncommitted work, nothing switched).
**F4** (a golden or `--lib` outcome outside spec §5.12 moves) → fix; not fixed in one attempt
→ stop at Task 1. **F1/F2/F5/F6** still apply if T1/T7/T8 change between Task 1 and Task 2
(they should not: Task 2 changes production only).

## Task 3 — the deletion, CHANGELOG, numbers-file skeleton (the LAST code commit)

Spec §4.11, §5.10, §5.12, §9 (discoveries). Depends on Task 2 and on Step 0's green record.

**Files:** `src/program/model/program_calls.rs` (delete J1's program and model sides,
`texts`, `span_text`, `op_name`, `by_decl` and its span, the surface lookup in
`l3_routine`, `span_join_under_demand`, `key_join_parity`, `j2_differences`; rewrite the module
doc's "Join keys"; move `model_key`/`program_key` here for the debug assertion),
`src/program/model/site_links.rs` (deleted) and `src/program/model/mod.rs`,
`src/program/model/events.rs` (delete `node_by_span` and the dependency surface pass; the
`_with_row_nodes` variant becomes `program_event_graph`), the oracle tests (T1, T7, T8 and
their CDO variants, deleted), `links_keep_every_edge_and_qualify_by_app` (deleted),
`can_fire_trigger_admits_the_resolver_ops` (its `op_name` half goes, spec §5.3),
`CHANGELOG.md`, `docs/measurements/2026-10-2x-issue-160-steps10-11a.md` (new skeleton).

- [ ] **Step 0: The licensing run (no commit).** On Task 2's commit, the same
  `agentflow run --name cdo-gate` command as Task 2 Step 0. Required: exit 0, T1/T7/T8 CDO
  lines unchanged, `adapter_cross_app_losses_on_cdo` green. Record under `## Gate results`.
  **Task 3 is committed only after this record exists and is green.**
- [ ] **Step 1: Delete** per spec §4.11. The permanent tests of spec §5.10 (T7a-c, T8a, A8)
  stay and stay green; no remaining test compares the new path with itself.
- [ ] **Step 2: Green.** `cargo test -p al-sem --lib`; `scripts/check-goldens`;
  `scripts/ci-steps clippy > <log> 2>&1` (release, `-D warnings`, no dead code left by the
  deletion); `rg -n 'span_text|op_name|SiteLinks|by_decl|node_by_span' src` empty (A10.3,
  A11a.3).
- [ ] **Step 3: Proofs re-run against the permanent tests:** T7b per-node selection, T8a
  `j5_node = node_of`, A8 break 8 → each fails; T5 (delete the span assertion) → fails (the
  assertion now lives in the only join).
- [ ] **Step 4: CHANGELOG** under `[Unreleased]` → `### Changed`: "#160 steps 10 + 11a" (Part
  of #160, no closing keyword): the key join and its speed-up (`<bench>` placeholders), the
  deleted joins, `RowNodes` and its compatibility answers, the collision census line, the new
  census buckets, amendments 10A-1..10A-5 and 11A-1..11A-4, no golden moved.
- [ ] **Step 5: Numbers-file skeleton**, shaped like the 7b one: protocol table (machine,
  parent = branch base, head from the bench's `shas.txt`, profile, harness, peaks via
  `maxrss.py`); Part 1 (histogram SHA, SARIF `cmp` both scopes both universes, collision line,
  census buckets head vs parent); Part 2 (B1 and B3 with void checks first, B2-a/B2-s, the
  single-app join reported); Part 3 (digest, fingerprint, 3 rounds, reported); the re-run rule
  and the verdict. Every number `<bench>`.
- [ ] **Step 6: Discoveries** (spec 10A-3) as ledger `## Discoveries` entries: (1) "body walk:
  XmlPort table elements are record receivers (18 Base Application sites on CDO)"; (2) "11b:
  J2's cross-node alias, J5's two mapping gaps and the collision group's 5 `NoProgramSite`
  sites" (with the `j2_alias_nodes` / `j5_compat_rows` counts from Task 2 Step 4's census
  line); (3) A8's precondition must be restated by 11b (spec §5.10).
- [ ] **Step 7: Gate and commit.** `scripts/ci-steps task-affected`, `scripts/check-goldens`;
  commit `#160 steps 10+11a Task 3: delete the span join, texts, J4, J2's and J5's per-use
  derivations`; ledger Timeline line and proof rows.

**Fallbacks that can fire here:** **F4** only (the deletion is mechanical once Tasks 1-2 are
green): a golden or `--lib` outcome moves → fix; not fixed in one attempt → blocked (Task 2's
commit stays; the deletion is not merged).

---

## The CDO bench (conductor only)

The three-part bench of spec §8 runs **once, after the final panel**, on the branch head
(Task 3's commit), through `agentflow run --name bench --timeout 45 -- bash <part>.sh`, never
an implementer and never beside a build. Bands come only from that bench's parent runs. Part 2
runs 5 rounds per side; Part 3 runs 3 (Decision 58). Peaks are read with `maxrss.py`
(`VmHWM`), psutil reported beside them (spec §8).

**Fallbacks that fire at the bench:** **F3** (Part 1: SARIF or histogram differs) → a defect;
one fix and re-run of Part 1 and T1/T7/T8, else blocked. **F8** (a census bucket > 1.5 × its
estimate, or B2 missed) → `u32` indices in `RowNodes` and the packed `ProgramSite`, re-run
Parts 1-2 once; still missed → F10. **F9** (B1 or B3 void twice) → A10.8 FAILS, the claim is
blocked, nothing merges; no re-labelling. **F10** (a bound met-not-void but missed) → name the
miss, ONE profiling fix, re-run Part 2 once; still missed → FAILS, blocked. Any fix after the
bench is a new commit after Task 3 and re-runs every identity gate it could affect (spec §10
preamble). The conductor fills Task 3's `<bench>` placeholders in a docs-only commit after a
passing bench.

**F11** does not apply (Decision 57).

## Acceptance map (spec §6)

| Acceptance | Where |
|---|---|
| A10.1 | Task 1 (stamps), Task 2 (read) |
| A10.2 | Task 2 (T5) |
| A10.3 | Task 3 |
| A10.4 | Task 2 (T2, CDO residual), Task 1 (r0 cross-app) |
| A10.5 | every task (`edge_order_is_call_sites_then_triggers` unchanged) |
| A10.6 | Task 2 Step 3 (whole debug suite, J1 present) and Task 2 Step 0 (T1 CDO) |
| A10.7 | Task 2 Step 5 (T2's break) |
| A10.8, A10.9 | the bench (B1, B3, B2) |
| A10.10 | Task 1 (`FileIx`), Task 2 (caller check) |
| A11a.1, A11a.2 | Task 1 (`RowNodes`), Task 2 (T9) |
| A11a.3 | Task 1 (T7, T8), Task 3 (deletion) |
| A11a.4 | Task 1 (one table) |
| A11a.5 | Task 1 (`collision_census_counts_groups`), bench Part 1 (CDO line) |
| A11a.6 | every task (`collision_rules_hand_stated`, T1 conversion, goldens) |
| A11a.7 | no bench for 11a: `cdo-gate` records (Task 2/3 Step 0) and bench Part 1 identity |
| E2/I2 | every task (`check-goldens`), bench Part 1 |
| E14 | Task 2/3 Step 0 (`cdo-gate`) and the branch gate |
