# Issue #100: TestField Capability Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** `TestField` stops being a capability `read`; `Rename`/`LockTable` become table facts `rename`/`lock`; d2 prunes on its own presence bit.

**Architecture:** One op table (`table_commit::map_op`) serves L2 and the cone (the cone's wrapper is deleted). `cone_derived` gains `TOUCHES_PHYSICAL_TABLE`, set by one shared predicate; d2 reads it at its two pruning sites, d1 keeps `touches_db_derived`.

**Tech Stack:** Rust (`al-sem` crate), cargo tests (umbrella `--test gap`, `--lib`).

**Spec:** `.agent/issue-100/ledger.md` (Design items 1-4, "Where the bit and d2's terminal filter disagree", Acceptance matrix, Plan notes). The acceptance tests already exist and are red: `tests/gap/gap_issue100_testfield_capability.rs`, the test module at the end of `src/engine/l5/capability_query.rs`.

## Global Constraints

- `rustfmt <file>` per file, never `cargo fmt`. No `#[allow(`. Do not touch `.github/ scripts/ .claude/ CLAUDE.md .gitignore Cargo.toml tree-sitter-al .agent/`.
- `export TREE_SITTER_AL_PATH=U:/Git/al-call-hierarchy/tree-sitter-al` for every cargo command; run from the worktree.
- `Rename` maps to NO permission letter (#9). The `RecordOpType` vocabulary still records `TestField`.
- Golden moves: only the four kinds in the ledger's "Outputs that move" (fact removal, presence loss, witness replacement, fact addition), expected list r1a `ws-d21.l2` / `ws-member-call-codeunit.l2`, r3a3 `ws-d21.r3a3` / `ws-member-call-codeunit.r3a3`; `ws-d60` may move only by new facts or mechanism (c). Anything else is unexplained.
- The acceptance tests are committed WITH the implementation (Task 2's commit), never alone.

## Review Focus

1. A fact with no `resource_id` (unresolved table): both bits set, before the early return in `fold_fact`. Pinned: `non_temp_fact_without_resource_id_sets_both_bits` (exists).
2. A routine whose cone is incomplete: d2 still walks it (`Unknown`). Pinned by reusing the `presence` arm; Task 1 Step 3 forbids a separate arm.
3. `alsem fingerprint` / `--r4f-snapshot` output for a `Rename`/`LockTable`-only routine gains a table family with no permission line. Pinned: Task 2 Step 5 golden triage (ws-d60) plus A2b.
4. A policy with an op-less `resourceKind: table` predicate now matches `rename`/`lock` facts; bundled `policy-default.yaml` must not change behaviour. Pinned: A2c and the existing policy tests in Task 2 Step 4.
5. Dedup: a shorter temp `Count` beating a `CalcFields` under one key. Pinned: `a2f_dedup_variant_count_wins_witness_and_finding_stays` (exists).

---

### Task 1: d2's own presence bit

**Files:**
- Modify: `src/engine/l4/cone_derived.rs` (bit constants `:105-113`, `fold_fact` `:690-712`, accessors near `:451-454`)
- Modify: `src/engine/l5/capability_query.rs` (beside `touches_db_derived` `:115-117`; raw oracles `:60-87`)
- Modify: `src/engine/l5/detectors/d2.rs` (`:136`, `:390`)
- Test: `src/engine/l5/capability_query.rs` test module (exists); `tests/gap/gap_issue100_testfield_capability.rs` (`a2d_*`, `a2e_*`, `a2f_*`, exist)

**Interfaces:**
- Produces: `pub const TOUCHES_PHYSICAL_TABLE: u8 = 1 << 4;` and `pub fn fact_sets_physical_table_bit(f: &CapabilityFact) -> bool` in `cone_derived.rs`; `ConeDerivedStore::touches_physical_table(&self, routine_id: &str) -> bool`; `pub fn touches_physical_table_derived(store: &ConeDerivedStore, s: &FullRoutineSummary) -> EffectPresence` and the test-only raw oracle `touches_physical_table_of` in `capability_query.rs` (same shape and cfg as `touches_db_of`).

- [ ] **Step 1: Confirm red.** Run `cargo test -p al-sem --lib capability_query 2>&1` → compile errors naming the five missing symbols; `cargo test --test gap gap_issue100_testfield_capability::a2` → `a2d_d2_not_starved_by_temp_write_branch` and `a2e_temp_forwarded_to_var_param_write_is_pruned` FAILED.
- [ ] **Step 2: Implement the predicate and bit.** `fact_sets_physical_table_bit(f)` = `f.resource_kind == "table" && !(fact_is_known_temp(f) && f.op != "read")`. In `fold_fact`'s `"table"` arm set `TOUCHES_PHYSICAL_TABLE` when the predicate holds, BEFORE the `resource_id` early return (next to `TOUCHES_TABLE`). Doc comment: the rule, why reads stay (FlowField on a temp host; dedup-invariance: reads only `op`/temp, both in `inherited_fact_key`).
- [ ] **Step 3: Accessors.** `touches_physical_table` mirrors `touches_table`; `touches_physical_table_derived` calls the existing private `presence` arm (no new tri-state logic); the raw oracle calls `fact_sets_physical_table_bit` over the summary's direct and inherited facts, exactly as `touches_db_of` iterates them (one rule, one copy).
- [ ] **Step 4: d2.** `D2Policy::expand` (`d2.rs:136`) and the subscriber pre-filter (`d2.rs:390`) call `touches_physical_table_derived` instead of `touches_db_derived`. Update the comment at `:390-400` (Class A/C) to say the pre-filter now drops known-temp non-read-only subscribers, and why d1 keeps `touches_db_derived` (temp terminals deliberate, `d1_graph.rs:127-160`).
- [ ] **Step 5: Green.** `cargo test -p al-sem --lib capability_query` → all pass; `cargo test --test gap gap_issue100_testfield_capability::a2d gap_issue100_testfield_capability::a2e gap_issue100_testfield_capability::a2f` → all pass. (The A1/A2/A2b/A2c tests stay red until Task 2.)
- [ ] **Step 6: Discrimination proofs** (Edit, run, revert with Edit; record each in the ledger's proof table): (i) revert both d2 sites to `touches_db_derived` → `a2d_d2_not_starved_by_temp_write_branch` and `a2e_*` fail; (ii) predicate excludes every known-temp fact (reads too) → both `a2f_*` fail; (iii) set the bit after the `resource_id` early return → `non_temp_fact_without_resource_id_sets_both_bits` fails.
- [ ] **Step 7:** No commit (Task 2 commits with the remaining tests); `rustfmt` the three files.

### Task 2: One op table, `TestField` out, `rename`/`lock` in

**Files:**
- Modify: `src/program/body/capability/table_commit.rs` (`map_op` `:24-40`, doc `:16`)
- Modify: `src/engine/l4/capability_cone.rs` (delete `map_table_op` `:167-171`; call `table_commit::map_op` at `:610`; doc `:115`)
- Modify: `src/engine/gate/policy/predicate_fields.rs` (`CAPABILITY_OPS` `:103-122`)
- Modify: `CHANGELOG.md` (`## [Unreleased]`, `### Fixed`)
- Goldens: as listed in Global Constraints
- Test: `tests/gap/gap_issue100_testfield_capability.rs` (`a1_*`, `a2_*`, `a2b_*`, `a2c_*`, exist)

**Interfaces:**
- Consumes: Task 1's bit (A2b's d2 shapes and A2d's Rename/LockTable variant rely on d2 pruning with it).
- Produces: `map_op("TestField") == None`, `map_op("Rename") == Some("rename")`, `map_op("LockTable") == Some("lock")`; `CAPABILITY_OPS` contains `"rename"`, `"lock"`.

- [ ] **Step 1: Confirm red.** `cargo test --test gap gap_issue100` → the 6 `a1_*`/`a2_fact_exists_iff_db_touching_class`/`a2b_*`/`a2c_*` tests FAILED.
- [ ] **Step 2: `map_op`.** Remove `"TestField"` from the `read` arm; add `"Rename" => Some("rename")`, `"LockTable" => Some("lock")`. Rewrite the "not capability-relevant" comment: `TestField` is an in-memory value check (`op_classification.rs:65` `StateOnly`), the JIT residual (ledger Design), and that `Rename` has a capability op but no permission letter (#9).
- [ ] **Step 3: Delete the cone wrapper.** Remove `capability_cone::map_table_op` and its doc; the call at `:610` becomes `crate::program::body::capability::table_commit::map_op(&op.op)` (or a `use`). Fix the two docs naming the wrapper (`capability_cone.rs:115`, `table_commit.rs:16`).
- [ ] **Step 4: Policy vocabulary.** Add `"rename"`, `"lock"` to `CAPABILITY_OPS`. Run `cargo test -p al-sem --lib policy` → pass.
- [ ] **Step 5: Goldens.** Run `bash scripts/check-goldens > <log> 2>&1` without regen first to list what moves; every moved line must be one of the four kinds and on the expected list (dispatch `golden-diff-triager`); write the receipt `.agent/golden-triage.md` (CLAUDE.md golden-regen gate) and only then `bash scripts/check-goldens --regen`, then `bash scripts/check-goldens` → green. An unexplained line is a discovery; stop and report.
- [ ] **Step 6: Green.** `cargo test --test gap gap_issue100` → 13 passed; `cargo test -p al-sem --lib` → pass; `bash scripts/ci-steps task > <log> 2>&1; echo $?` → `0`.
- [ ] **Step 7: Discrimination proofs** (Edit, run, revert): (i) restore `"TestField"` in the `read` arm → both `a1_*` fail; (ii) remove the `Rename`/`LockTable` arms → `a2b_*` fail, and `a2d_d2_not_starved_by_temp_rename_lock_branch` stays green (pruned either way); (iii) add a private copy of the corrected table in the cone with one entry changed → `a2_l2_and_cone_agree_over_the_whole_vocabulary` fails; (iv) drop `"rename"` from `CAPABILITY_OPS` → `a2c_*` fails. Record each in the ledger.
- [ ] **Step 8: CHANGELOG** `### Fixed` entry per the ledger's CHANGELOG paragraph (TestField no longer a read; new `rename`/`lock` table facts with no permission letter and where they surface; d2's own pruning bit and mechanisms (e)/(f)/(g); `CapabilityLost` churn against older snapshots; narrower `op = read` policy matches; removed `TableData R` rows; the JIT residual).
- [ ] **Step 9: Commit.** `check-diff --base master --head HEAD` first (executor). Then:

```bash
git add src/engine/l4/cone_derived.rs src/engine/l5/capability_query.rs src/engine/l5/detectors/d2.rs \
  src/program/body/capability/table_commit.rs src/engine/l4/capability_cone.rs \
  src/engine/gate/policy/predicate_fields.rs tests/gap/gap_issue100_testfield_capability.rs tests/gap/main.rs \
  CHANGELOG.md <each regenerated golden path>
git commit -m "fix(capability): TestField is not a table read; Rename/LockTable facts; d2 prunes on its own physical-table bit (#100)"
```
