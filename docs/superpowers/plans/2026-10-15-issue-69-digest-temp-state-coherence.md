# Issue #69 — Digest Temp-State Coherence Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Inside one digest document, every statement about a DB write's temp state agrees and is true in the reporting routine's frame; `Validate`/`Copy`/`TransferFields` stop being DB writes.

**Architecture:** Producers (raw direct table facts whose temp state is parameter-dependent) leave shared cone propagation. A transitive product-state solve over the typed edges resolves them per routine (D1). The snapshot carries the solve as an arena (D2). The digest witness reads root-owned arena proofs plus today's ordinary-carrier BFS (D3). `detail.tempState` becomes a per-operation, class-filtered join with an incoherence diagnostic (D4). One op map for L2 and the cone (D5).

**Tech Stack:** Rust (crate `al-sem`), libtest umbrella crates, byte-compared goldens.

**Spec:** `docs/superpowers/specs/2026-10-14-issue-69-digest-temp-state-coherence-design.md` (rev 5b + R6-F01). It is the contract. Read D1..D5, the acceptance matrix and the measurement plan before your task. Every `file:line` below is from base `049b166c` and drifts as tasks land; find the code by the named function, not the number.

## Global Constraints

- Worktree: `U:\Git\al-sem-issue-69-a1`, branch `issue/69-digest-ordering-reads-the-cone-a1`. Base B0 = `049b166cd4f93d28b83e56159da60e11fa787327`.
- Every cargo command runs with `TREE_SITTER_AL_PATH=U:/Git/al-call-hierarchy/tree-sitter-al` and with `CDO_WS` UNSET (`env -u CDO_WS`).
- Package name is `al-sem` (hyphen): `cargo test -p al-sem ...`.
- Protected paths, never touched by any task: `.github/`, `scripts/`, `.claude/`, `CLAUDE.md`, `.gitignore`, `Cargo.toml` version fields, `tree-sitter-al/`. `.agent/` is written (ledger, triage receipt) but NEVER staged or committed.
- Edit source, tests, docs and CHANGELOG with the Edit/Write tools only. No `sed -i`, no Python rewrites. Generated files (golden regen, `rustfmt`) are the only exception.
- Format each touched `.rs` file with `rustfmt <file>`. Never `cargo fmt`.
- Stage explicit paths only. Never `git add -A`. One commit per task. Never push, never merge.
- Never pipe a gate through `| tail`. Redirect to a log and grep it: `scripts/ci-steps task > .agent/issue-69/task-N-gate.log 2>&1; echo rc=$?`.
- A task's commit must pass `scripts/ci-steps task` (fmt, dev clippy `-D warnings`, gen-syntax, whole suite). The pre-commit hook re-runs `check-goldens` for any `src/` change; it is skipped only when the tree equals the stamp a green `ci-steps test` wrote.
- Spec constants are unchanged: `HARD_PATH_CAP = 256`, `MAX_DEPTH = 64`, `MAX_STATES = 25_000` (`src/engine/l5/digest.rs:386-388`), `MAX_PATHS` as today.
- Schema bumps (D4, additive minor): digest `schemaVersion` `1.5.0` → `1.6.0` (`digest_cli.rs:1259`); prove `1.3.0` → `1.4.0` (`prove.rs:725`).
- New diagnostic kind string: `temp-state-incoherent`; reasons `no-l4-row` | `class-absent`.
- Detector, ordering and digest semantics may move ONLY in the directions the spec names. Every moved golden line is triaged (see "Golden protocol").

## How the red tests ride (decision)

The acceptance tests already exist, uncommitted. The pre-commit hook builds the WORKING TREE, so uncommitted tests that do not compile (A4b, A5 Compare 1) would block every commit.

**Decision (revised 2026-10-15 after Task 1's `check-diff` STOP): no `#[ignore]` and no `#[allow(...)]` ever enter the tree — the executor forbids both, by doctrine. A still-red acceptance test is PARKED: cut verbatim out of the compiled tree into `.agent/issue-69/parked/<taskN>-<destination>.rs.txt` (never committed), and the task that greens it RESTORES it in the same commit as the code.** Each parked file starts with a `//` header saying where its block goes and which imports to add; restore = paste everything below the header, then follow the header. Task 1 commits only the tests green after D5 (A4, A1b, A3) and the probe workspace `tests/common/temp_state_probe.rs` (no `allow`; it carries only what the committed tests use). `line_of` is parked in the Task-4 cli file, not the probe module: only the cli umbrella reads `// @marker` lines, so in the `r3` umbrella it would be dead code.

Parked files (the test → the task that restores it):

| Parked file (`.agent/issue-69/parked/`) | Destination | Contents | Restored in |
|---|---|---|---|
| — (committed in Task 1) | cli | `digest_db_effects_carry_query_effects_temp_state` (A4), `a1b_no_temp_state_incoherence` (green on base; its proof needs D4), `a3_state_only_ops_are_not_db_writes` | — |
| `task2-cli_digest_temp_state.rs.txt` | `tests/cli/cli_digest_temp_state.rs` | `a1c_cone_values_are_frame_correct` + `snapshot_writes` (+ snapshot/model imports) | 2 |
| `task2-r3_temp_class_parity.rs.txt` | `tests/r3/r3_temp_class_parity.rs` (whole file) + `tests/r3/main.rs` (`temp_state_probe` and `r3_temp_class_parity` mod lines) | `a5_cone_l4_class_parity_reduced` (A5 Compare 2) | 2 |
| `task3-r3_temp_class_parity_arena.rs.txt` | `tests/r3/r3_temp_class_parity_arena.rs` (whole file) + its `mod` line | `a5_cone_l4_class_parity_per_producer` (A5 Compare 1) | 3 |
| `task4-cli_digest_temp_state.rs.txt` | `tests/cli/cli_digest_temp_state.rs` | `line_of`, the A2 reading helpers (`is_kt`, `is_nt`, `has_guarantee`, `via_paths`, `path_str(s)`, `hop_lines`, `evidence_kind`, `the_kt_insert`), `a2a_*`, `a2d_*`, `a2g_*`, `a2f_*` | 4 |
| `task5-cli_digest_temp_state.rs.txt` | `tests/cli/cli_digest_temp_state.rs` | `terminal`, `a1a_*`, `a2b_*`, `a2c_*`, `a2e_*` + `mixed_carrier_precondition` | 5 |
| `task5-digest_cli_tests.rs.txt` | `src/engine/l5/digest_cli.rs` `mod tests` | A4b `temp_join_*` (2 tests) | 5 |

A2e's six claims that are already green on the base (RecH, RootCyc1, GuardFwdFalse/True, GCycTrue, GTTrue) ride with their row: they share A2e's closures, and their named reds come only from mutations of new code.

Why A1a/A2b/A2c/A2e wait for Task 5: each has a claim on an operation reached in BOTH classes (`HelperUiBoth`, `HelperUiBothDeep`, `PermMix`, `SelfRecReal`), and today's `for_effect` unions both classes into each effect's `tempState`. Only D4's class filter makes those claims checkable.

**Parking rule.** Task N restores ITS parked files first (red), then greens them. Before committing, run `cargo test -p al-sem --test cli cli_digest_temp_state::` (and, from Task 2, `cargo test -p al-sem --test r3 r3_temp_class_parity`) and paste the summary into the ledger: everything compiled must pass. A later task's parked test may be pasted in temporarily to measure it, and must be cut back out before the commit. If a restored test is still red only on lines that provably need a LATER task's code, re-park it under that task's file name, quote the remaining red lines and the reason in the ledger, and get the reviewer to agree. No parked file is ever staged.

## Discrimination proof protocol (every task)

For each proof the task owes: (1) make the named mutation with Edit; first `grep -c` the exact old text and assert the count is 1, so the break provably applied; (2) run the named test command; it must fail ON THE NAMED ASSERTION TEXT (quote the line), not on a compile error or an unrelated assertion; (3) revert with Edit; (4) re-run, green. Record in `.agent/issue-69/ledger.md` under "## Discrimination proofs": row id, mutation, file:function, red line quoted, green confirmation. A proof that stays green when broken is a test defect: stop and diagnose (see CLAUDE.md "A discrimination proof that PASSES is evidence about the TEST").

## Golden protocol (every task that changes `src/`)

1. Run `scripts/check-goldens > .agent/issue-69/task-N-goldens.log 2>&1; echo rc=$?` WITHOUT regen first. This lists what moved.
2. Write the receipt `.agent/golden-triage.md` (must be < 90 minutes old at regen time; one `MOVED: <path>` line per moved file, each path must exist). Under each, one line per moved hunk: the spec cause (D1 value / D3 witness / D3 identity / D4 / D5 / truncation widening / order re-sort R6-F01) and why it is the correct direction. A line no spec cause explains is a BUG: stop and fix the code, do not regen it in. The `golden-diff-triager` agent may be used to classify hunks; it never blesses.
3. `scripts/check-goldens --regen > ... 2>&1`, inspect `git diff --stat tests/`, then `scripts/check-goldens` again (no regen): green.
4. Must NEVER move in any task: `tests/l4-summary-baseline/`, `tests/goldens/semantic-edges/`, `tests/r0-goldens/`, `tests/r2*-goldens/`, `tests/ir-l2-goldens/`. If one moves, the change is wrong.
5. Copy the receipt's content into the ledger under "## Gate results / task N" (the receipt file itself is overwritten by the next task).

## Review Focus

1. **A workspace with no producers at all** (most real apps have few PD writes): the solve must cost ~nothing and the arena must be empty, with byte-identical output for routines that never reach a producer. Pinned by Task 2 Step 8 (r0-corpus goldens of fixtures without `var` record writes must not move; check-goldens shows this).
2. **Event-dispatch edges between a publisher and a subscriber whose `var` record parameter is a producer** (the `event_param_temp_state` branch of `cross_edge`): an incorrect by-name parameter map silently classes a real write as temporary. Pinned by Task 2 Step 1's unit test `cross_edge_event_dispatch_maps_by_name`.
3. **A producer reached through a cycle with no exit to a caller** (`CycA`/`CycB` shape) must still terminate: the visited set is the state tuple, never the routine. Pinned by A2e `CycA` plus Task 2's unit test `solve_terminates_on_cycle`.
4. **Cross-app mode** (`single_app = false`, the default): the arena is built from the source-only snapshot (`compose_snapshot`), and dependency routines never get states. Every cli test runs both modes; Task 3's `debug_assert` counts cone edges with no snapshot edge.
5. **A digest root that is itself the producer's owner** (the seed): the seed is never an arena carrier for the owner's own direct fact (that fact is `direct` and takes Case A of the witness). Pinned by Task 4 Step 1's unit test `seed_state_is_not_a_proof_carrier`.

---

### Task 1: Land the acceptance tests and unify the op map (D5)

**Files:**
- Modify: `src/program/body/capability/table_commit.rs:17-31` (`map_op`: drop `Validate`/`Copy`/`TransferFields`; make it `pub(crate)`)
- Modify: `src/program/body/capability/mod.rs` (re-export so the cone can call it, if `table_commit` is private)
- Modify: `src/engine/l4/capability_cone.rs:162-174` (`map_table_op` becomes a call to `table_commit::map_op`)
- Modify: `src/engine/l5/digest_cli.rs` (cut the A4b block out; park it — net byte-identical to B0)
- Park (see "How the red tests ride"): every red test in `tests/cli/cli_digest_temp_state.rs`, the whole of `tests/r3/r3_temp_class_parity.rs` and `tests/r3/r3_temp_class_parity_arena.rs`, and their `tests/r3/main.rs` lines (net byte-identical to B0)
- Commit: `tests/common/temp_state_probe.rs` (no `#![allow(dead_code)]`, no `line_of`), `tests/cli/main.rs`, `tests/cli/cli_digest_temp_state.rs` (A4, A1b, A3 and their helpers only)
- Modify: `CHANGELOG.md` (`### Fixed` under Unreleased)
- Goldens: expected to move `tests/r1a-goldens/`, `tests/r3a3-goldens/`, `tests/r3a5-goldens/`, `tests/r4f-goldens/`, `tests/cli-*-goldens/`, `tests/gate-goldens/`, possibly `tests/r4-goldens/`.

**Interfaces:**
- Produces: `pub(crate) fn crate::program::body::capability::table_commit::map_op(op: &str) -> Option<&'static str>` — the ONE table op map. `capability_cone::map_table_op` delegates to it (keep the wrapper name so its call sites do not change).

**Acceptance rows greened:** A3.

- [ ] **Step 1: Park the A4b tests.** Copy the A4b block (in `src/engine/l5/digest_cli.rs` `mod tests`, from `// --- #69 D4: the per-operation temp-state join (acceptance row A4b) -----` through the closing `}` of `temp_join_no_l4_row_makes_the_field_absent`, including `use crate::engine::l4::effect_lattice::TempStateKind;` and `fn ops`) to `.agent/issue-69/parked/task5-digest_cli_tests.rs.txt`, then delete it from `digest_cli.rs` with Edit.

- [ ] **Step 2: Park every other red test** into the parked files listed in "How the red tests ride" (A1c/A5/A2*/A1a, with the helpers only they use, `line_of` into the Task-4 cli file). Remove `#![allow(dead_code)]` from `tests/common/temp_state_probe.rs`; the module must then build warning-free in the `cli` umbrella. Remove the `temp_state_probe` / `r3_temp_class_parity*` lines from `tests/r3/main.rs`.

- [ ] **Step 3: Confirm A3 is red on the base.**

Run: `cargo test -p al-sem --test cli cli_digest_temp_state::a3_state_only_ops_are_not_db_writes`
Expected: FAIL with `ValidateTemp: DB_MODIFY present` (and the other eight routines, both modes).

- [ ] **Step 4: Unify the op map.** In `table_commit.rs`:

```rust
/// Map an AL `RecordOpType` to a `CapabilityOp`, or `None` for state-only /
/// filter ops. The ONE table op map: `capability_cone::map_table_op` calls it.
///
/// `Validate`, `Copy` and `TransferFields` are NOT writes (#69 D5): `Copy`
/// copies fields/filters/keys in memory, `TransferFields` assigns field values,
/// `Validate` assigns and runs `OnValidate` (whose own writes are separate
/// effects; its relation check reads the RELATED table). None of them modifies
/// the record's own table.
pub(crate) fn map_op(op: &str) -> Option<&'static str> {
    match op {
        "Get" | "Find" | "FindFirst" | "FindLast" | "FindSet" | "IsEmpty" | "Count"
        | "CountApprox" | "Next" | "CalcFields" | "CalcSums" | "TestField" => Some("read"),
        "Modify" | "ModifyAll" => Some("modify"),
        "Insert" => Some("insert"),
        "Delete" | "DeleteAll" => Some("delete"),
        // Rename: deliberately unmapped (#9) — see the old comment, keep it.
        _ => None,
    }
}
```

Keep the existing Init/SetRange/Rename comment lines. In `capability_cone.rs` replace the body of `map_table_op` with `crate::program::body::capability::table_commit::map_op(op)` (make the module path reachable with `pub(crate) mod table_commit;` in `capability/mod.rs` if needed). Grep for any other copy of the `"Validate" | "Copy" | "TransferFields"` arm in `src/` (`grep -rn '"TransferFields"' src`); the spec names exactly two maps. Do not touch `summary_runner.rs`/`op_classification.rs` (they already classify these as non-writes).

- [ ] **Step 5: Run A3.** Same command as Step 3. Expected: PASS.

- [ ] **Step 6: Discrimination proofs (A3, three).** One at a time, put `"Validate"`, then `"Copy"`, then `"TransferFields"` back into the `"Modify" | "ModifyAll"` arm of `table_commit::map_op`. Expected red: `ValidateTemp: DB_MODIFY present`, `CopyTemp: DB_MODIFY present`, `TransferTemp: DB_MODIFY present` respectively. Revert each, green.

- [ ] **Step 7: Goldens** (Golden protocol). Expected causes: D5 only — a `modify` capability fact, `DB_MODIFY` effect, fingerprint `M`, diff `CapabilityGainedWrite`, policy `modify`, d43/d44/d45 physical-write findings that came from a Validate/Copy/TransferFields site disappear. `tests/r1a-goldens/` must move (9 fixtures carry these ops). Anything else is a bug.

- [ ] **Step 8: CHANGELOG.** Under `## [Unreleased]` → `### Fixed`: "`Validate`, `Copy` and `TransferFields` are no longer reported as database writes, on temporary and real records alike (#69). Moved consumers: capability facts (no `modify`), digest/prove (no `DB_MODIFY`), physical-write sets of d43/d44/d45, fingerprint `M`, diff `CapabilityGainedWrite`, policy `modify`. The #65 digest test that pinned `ValidateTemp` emitting `DB_MODIFY` is replaced by the opposite assertion." Under `### Changed`: "L2 capability extraction and the capability cone share one table-op map (`table_commit::map_op`)."

- [ ] **Step 9: Gate and commit.**

Run: `scripts/ci-steps task > .agent/issue-69/task-1-gate.log 2>&1; echo rc=$?` → `rc=0`; then the Parking rule command.

```bash
git add src/program/body/capability/table_commit.rs \
  src/engine/l4/capability_cone.rs \
  tests/common/temp_state_probe.rs tests/cli/main.rs tests/cli/cli_digest_temp_state.rs \
  CHANGELOG.md <every regenerated golden path>
git commit -m "#69 D5: Validate/Copy/TransferFields are not DB writes; one table op map; land the green #69 acceptance tests"
```

(`src/engine/l5/digest_cli.rs` and `tests/r3/main.rs` are byte-identical to B0 after parking; do not stage them.)

---

### Task 2: D1 — the transitive producer solve in the cone

**Files:**
- Create: `src/engine/l4/pd_solve.rs` (state, `cross_edge`, the solve, `ResolvedEntry`, the internal arena)
- Modify: `src/engine/l4/mod.rs` (`pub mod pd_solve;`)
- Modify: `src/engine/l4/capability_cone.rs`:
  - `ConeFactEntry` (`:1396`): delete `pd`; delete `frame_bound`'s PD half (`frame_bound` becomes `entry.guard.is_some()`), `fact_pd_index` stays (the solve uses it).
  - `substitute_entry` (`:1517`): delete the temp half; the guard half becomes a call to `pd_solve::carry_reqs` (shared with `cross_edge`).
  - `merge_cone` (`:1634`): delete the `pd` agreement logic.
  - `fact_cone_for_scc` (`:2059`): producers never enter; delete `entry_pd`.
  - `PdAnchor` (`:2262`): delete the PD-only fields (keep what the guard machinery reads; rename to `GuardAnchor`).
  - `compose_cone_over_graph` (`:2671`): split producers out of `direct` (D1.1), run the solve, pass `resolved_by_routine` to `compose_inherited_cones`.
  - `inherited_facts_for_singleton` (`:1735`) and `inherited_facts_by_bfs` (`:1930`): use (a) and (b); the BFS no longer emits a sibling's producers.
  - `ConeOutcome`: add `pub pd_solution: pd_solve::ConeArena`.
- Restore (parked): `task2-cli_digest_temp_state.rs.txt` into `tests/cli/cli_digest_temp_state.rs`; `task2-r3_temp_class_parity.rs.txt` as `tests/r3/r3_temp_class_parity.rs` plus its two `tests/r3/main.rs` mod lines. Do this FIRST (before Step 1) and record the red run in the ledger.
- Modify: `CHANGELOG.md`

**Interfaces:**
- Consumes: `summary_runner::pd_temp_state_at_callsite(caller, cs, i) -> TempState`, `summary_runner::event_param_temp_state(publisher, subscriber, i) -> TempState`, `param_guard::{across_edge, conjoin, FrameGuards, Req}`, `capability_cone::{TypedOutEdge, TypedEdgeGraph, site_reqs, guard_frames, retag, rep_key, inherited_fact_key, fact_pd_index}`.
- Produces (crate-internal, read by Task 3 and Task 4):

```rust
// src/engine/l4/pd_solve.rs
use std::sync::Arc;
use super::capability_cone::{CapabilityFact, TypedOutEdge};
use super::param_guard::Req;
use super::summary::TempState;

/// Index into `ConeArena::states`.
pub type StateIx = usize;

/// One solve state `(p, r, t, reqs)`: in routine `routine`'s frame, producer
/// `producer`'s write has temp state `t` and happens only under `reqs`.
#[derive(Debug, Clone)]
pub struct ConeState {
    /// Index into `ConeArena::producers`.
    pub producer: usize,
    /// Internal routine id (the cone's id, NOT the stable id).
    pub routine: Arc<str>,
    pub t: TempState,
    pub reqs: Vec<Req>,
    /// True only for the seed `(p, owner(p), PD(i0), site reqs)`.
    pub seed: bool,
    /// Shortest link-path length to the seed (seed = 0).
    pub dist: usize,
    /// Range into `ConeArena::links`: this state's incoming links, deduplicated,
    /// in edge order (`edge_sort_key`).
    pub links: std::ops::Range<usize>,
}

/// A link: "this state is reached from `callee_state` across `edge`"
/// (`edge.from == state.routine`, `edge.to == callee_state.routine`).
#[derive(Debug, Clone)]
pub struct ConeLink {
    pub callee_state: StateIx,
    pub edge: TypedOutEdge,
}

#[derive(Debug, Clone)]
pub struct Producer {
    /// The owner's raw direct fact (PD temp state, `provenance == "direct"`).
    pub fact: Arc<CapabilityFact>,
    pub owner: Arc<str>,
    pub operation_id: String,
}

/// The whole solve, in cone ids. Task 3 maps it into the snapshot.
#[derive(Debug, Clone, Default)]
pub struct ConeArena {
    pub producers: Vec<Producer>,
    pub states: Vec<ConeState>,
    pub links: Vec<ConeLink>,
    /// Internal routine id → its states (seed included).
    pub by_routine: std::collections::HashMap<Arc<str>, Vec<StateIx>>,
}

/// D1.3: one non-seed state placed at its routine for uses (a) and (b).
#[derive(Debug, Clone)]
pub struct ResolvedEntry {
    pub state: StateIx,
    /// The producer's raw fact with `temp_state = t` and
    /// `witness_operation_id` = the producer's op. One `Arc` per
    /// `(producer, t)`, shared by every state with that pair.
    pub rep: Arc<CapabilityFact>,
    pub dist: usize,
    /// The edge of this state's shortest link (r's out-edge).
    pub first_hop: TypedOutEdge,
    pub reqs: Vec<Req>,
}

pub type ResolvedByRoutine = std::collections::HashMap<Arc<str>, Vec<ResolvedEntry>>;

/// The guard step shared by `cross_edge` and `capability_cone::substitute_entry`:
/// `across_edge` on call edges, the by-name re-anchor on event edges, then
/// `conjoin` with `edge_reqs`. `None` = contradiction (drop the transition).
pub(crate) fn carry_reqs(
    reqs: &[Req],
    edge: &TypedOutEdge,
    caller: Option<&ModelRoutine>,
    callee: Option<&ModelRoutine>,
    caller_frame: Option<&FrameGuards>,
    edge_reqs: &[Req],
) -> Option<Vec<Req>>;

/// D1.2 transition over typed edge `caller → callee`. EXACTLY today's per-edge
/// step of `substitute_entry`, factored out: the temp map applies to PD only
/// (`pd_temp_state_at_callsite` on direct-call/variable-typed-call,
/// `event_param_temp_state` on event-dispatch, `Unknown` on any other kind);
/// known/unknown transfer unchanged; reqs go through `carry_reqs`.
pub(crate) fn cross_edge(
    t: &TempState,
    reqs: &[Req],
    edge: &TypedOutEdge,
    caller: Option<&ModelRoutine>,
    callee: Option<&ModelRoutine>,
    caller_frame: Option<&FrameGuards>,
    edge_reqs: &[Req],
) -> Option<(TempState, Vec<Req>)>;

/// The transitive product-state solve over the typed graph's INCOMING edges.
pub(crate) fn solve(
    producers: Vec<Producer>,
    g: &TypedEdgeGraph,
    routines_by_id: &HashMap<&str, &ModelRoutine>,
    frames: &HashMap<String, FrameGuards>,
) -> (ConeArena, ResolvedByRoutine);
```

`TempState` needs `Clone + Eq + Hash` (add derives in `summary.rs` if missing). `TypedOutEdge`, `TypedEdgeGraph`, `site_reqs`, `edge_sort_key` become `pub(crate)` in `capability_cone.rs`. If `TypedEdgeGraph` has no incoming index, build `incoming: HashMap<&str, Vec<&TypedOutEdge>>` inside `solve` once.

**Acceptance rows greened:** A1c; A5 Compare 2.

- [ ] **Step 1: Unit tests first (red: module missing).** In `pd_solve.rs` `#[cfg(test)] mod tests`, hand-state the inputs (no pipeline):
  - `cross_edge_pd_maps_only_pd`: `t = Known(true)` across a `direct-call` edge stays `Known(true)` whatever the argument; `t = ParameterDependent(0)` across a `direct-call` whose call site passes a local temporary record becomes `Known(true)`.
  - `cross_edge_other_kind_is_unknown`: PD across an edge of kind `"object-run"` (any kind not in the three) becomes `Unknown`.
  - `cross_edge_event_dispatch_maps_by_name` (Review Focus 2): publisher params `(A, B)`, subscriber params `(B, A)`, subscriber PD(0) (= `B`) maps to the publisher's index 1.
  - `cross_edge_drops_on_contradiction`: state `reqs = [(0, true)]`, the caller passes literal `false` for param 0 → `None`.
  - `solve_terminates_on_cycle` (Review Focus 3): two routines calling each other forwarding the `var` param; assert `solve` returns and both routines hold exactly the PD state of their own frame.
  - `solve_keeps_all_links`: one caller with two call sites into the producer's owner → the caller's state has 2 links, in `edge_sort_key` order.
  Build `ModelRoutine`/`TypedOutEdge` values literally; copy the construction pattern of the existing `param_guard.rs` tests.

Run: `cargo test -p al-sem --lib pd_solve` → FAIL (does not compile / missing fns).

- [ ] **Step 2: Implement `carry_reqs` and `cross_edge`** by MOVING the body of `substitute_entry` (`capability_cone.rs:1517-1622`): the `event`/`binds`/`site` computation, the `pd_state` match (now on `t` instead of `entry.pd`), and the guard block. `substitute_entry` keeps only `carry_reqs(&g.reqs, edge, caller, callee, caller_frame, edge_reqs)` for ordinary guarded entries (empty reqs when unguarded, then `conjoin` with `edge_reqs`, as today).

- [ ] **Step 3: Implement `solve`.** Pattern: `db_effect_solver::solve_pd_reachability` (`db_effect_solver.rs:220`).

```rust
pub(crate) fn solve(
    producers: Vec<Producer>,
    g: &TypedEdgeGraph,
    routines_by_id: &HashMap<&str, &ModelRoutine>,
    frames: &HashMap<String, FrameGuards>,
) -> (ConeArena, ResolvedByRoutine) {
    // key = (producer, routine, t, reqs): the visited set is the observed tuple.
    type Key = (usize, Arc<str>, TempState, Vec<Req>);
    let mut arena = ConeArena { producers, ..Default::default() };
    let mut ix_of: HashMap<Key, StateIx> = HashMap::new();
    let mut raw_links: Vec<Vec<ConeLink>> = Vec::new();
    let mut queue = std::collections::VecDeque::new();
    for (p, prod) in arena.producers.iter().enumerate() {
        let i0 = fact_pd_index(&prod.fact).expect("producer is PD");
        let reqs = frames
            .get(&*prod.owner)
            .and_then(|fg| site_reqs(fg, &prod.fact))
            .cloned()
            .unwrap_or_default();
        // push seed state (seed: true, dist 0), enqueue it
    }
    // BFS by dist: pop state s at routine c; for every typed edge e with e.to == c
    // (sorted by edge_sort_key), caller r = e.from:
    //   let caller = routines_by_id.get(r); let callee = routines_by_id.get(c);
    //   let edge_reqs = frames.get(r).and_then(|f| e.callsite.as_deref()
    //        .and_then(|cs| f.by_site.get(cs))).map_or(&[][..], Vec::as_slice);
    //   if let Some((t2, reqs2)) = cross_edge(&s.t, &s.reqs, e, caller, callee, frames.get(r), edge_reqs)
    //   then intern (p, r, t2, reqs2): new → push with dist = s.dist + 1, enqueue;
    //   either way record link {callee_state: s, edge: e.clone()} on the target,
    //   deduplicated.
    // NO state stops: known/unknown states propagate too (rev 4).
    // Finally: flatten raw_links into arena.links (each state's links sorted by
    // edge_sort_key, then callee_state), set ranges, fill by_routine (seed included),
    // and build ResolvedByRoutine: one ResolvedEntry per NON-seed state, rep shared per
    // (producer, t) via a HashMap<(usize, TempState), Arc<CapabilityFact>>,
    // first_hop = the edge of a link whose callee_state has dist == s.dist - 1
    // (first in link order).
    (arena, resolved)
}
```

The rep for `(p, t)`: clone `prod.fact`, set `extra`'s table `temp_state` to `t` rendered as a `PTempState` (`known` + value / `parameter-dependent` + index / `unknown`), keep `witness_operation_id = prod.operation_id`, provenance stays `"direct"` (`retag` makes it inherited at use (a)).

- [ ] **Step 4: Split producers (D1.1).** In `compose_cone_over_graph`, a raw fact is a producer iff `fact_pd_index(f).is_some()` and `f.witness_operation_id.is_some()`. Build `direct` (the dedup map) from ordinary facts only. Producers stay in `direct_in`/`direct_raw` (the owner's SELF fold reads PD as nt, which is right in the owner's own frame) and go to `pd_solve::solve`. Pass `&resolved` into `compose_inherited_cones` and on to both walks. Return the arena in `ConeOutcome.pd_solution`.

- [ ] **Step 5: Uses (a), (b), (c).**
  - (a) In `inherited_facts_for_singleton` and `inherited_facts_by_bfs`, after the existing candidates, add each `ResolvedEntry` of `subject` as a candidate `(rep, dist, first_hop)` under the same min-dist / edge-sort rule per base key (`inherited_fact_key(rep)` + `guarded_key` with `reqs`). This applies to EVERY SCC shape, root SCCs and recursive members included.
  - (b) The same candidates go into the derived fold exactly like a guarded/unguarded inherited rep today.
  - (c) Delete the branch in `inherited_facts_by_bfs` that emits a sibling member's direct facts when they are producers (`:1977-1985`): sibling producers are now that routine's own entries. `fact_cone_for_scc` never sees producers because `direct` no longer holds them.
  - Delete `ConeFactEntry.pd`, the PD half of `merge_cone`, `entry_pd`, and the PD fields of `PdAnchor`. `cargo build` must show no dead-code warnings; delete what becomes dead.

- [ ] **Step 6: Run the unit tests and A1c / A5 Compare 2.** (Restored from the Task-2 parked files at the start of the task.)

Run: `cargo test -p al-sem --lib pd_solve` → PASS.
Run: `cargo test -p al-sem --test cli cli_digest_temp_state::a1c_cone_values_are_frame_correct` → PASS.
Run: `cargo test -p al-sem --test r3 r3_temp_class_parity::a5_cone_l4_class_parity_reduced` → PASS.

If Compare 2 still shows lines, each is either a real solve bug (fix it) or a GUARDED-fixture superset (the list is explicit in the test; do not widen it without a spec reason recorded in the ledger).

- [ ] **Step 7: Discrimination proofs.**
  - A1c: in `solve`'s rep construction, use the producer's raw PD temp state instead of `t` → red `ViaHelperReal: PD(0) != known/false`.
  - A5 Compare 2: in `solve`, skip transitions where `scc_id(e.from) == scc_id(e.to)` (pass the SCC map in for the mutation only, or compare via a recursive-member check you add temporarily) → red on a line naming `RecUi2` in `a5_cone_l4_class_parity_reduced`. Record the exact line.

- [ ] **Step 8: Goldens.** Expected causes: D1 values (PD in a caller's frame becomes known(true)/known(false)/unknown/PD(j)), producer separation (a producer no longer shares a key with an ordinary fact, so an effect can split), guards on PD-origin facts in recursive SCCs, tie-break moves from min-dist candidates. `tests/r3a3-goldens/`, `tests/r3a5-goldens/`, `tests/r4f-goldens/`, `tests/cli-*`, `tests/r4-goldens/` may move. Fixtures without `var` record writes must be byte-identical (Review Focus 1); confirm by listing which fixtures moved and grepping each for a `var` record parameter.

- [ ] **Step 9: CHANGELOG.** `### Fixed`: "The capability cone resolves a parameter-dependent write in every routine's own frame through a transitive per-producer solve (#69 D1). A write a caller makes temporary is graded known(true), a real record known(false), across recursion, cycles and root SCCs too; guards on these writes now hold on every typed edge, cycles included. Producers no longer share a cone key with ordinary facts." `### Removed`: "The frame-bound parameter-dependent cone entry (`ConeFactEntry.pd`) and its singleton-only substitution."

- [ ] **Step 10: Gate and commit.** `scripts/ci-steps task` (log + rc), Parking rule, then:

```bash
git add src/engine/l4/pd_solve.rs src/engine/l4/mod.rs src/engine/l4/capability_cone.rs \
  src/engine/l4/summary.rs tests/cli/cli_digest_temp_state.rs tests/r3/main.rs tests/r3/r3_temp_class_parity.rs \
  CHANGELOG.md <regenerated golden paths>
git commit -m "#69 D1: transitive producer solve; producers leave shared cone propagation"
```

---

### Task 3: D2 — the solve arena on the snapshot, census counters, A5 Compare 1

**Files:**
- Modify: `src/engine/l5/snapshot.rs` (`CapabilitySnapshot` `:813`, `compose_snapshot` `:1044`, `derive_typed_edges` `:1206`)
- Modify: the snapshot's base builder that calls `compose_cone_over_graph` (`capability_cone.rs` `build_r3a3_source_only_base` `:3872` / `R3a3SourceBase`): carry `pd_solution` through
- Modify: `src/engine/l5/detector_context.rs` (`cones_census`, `:344`): new counters + report lines
- Restore (parked): `task3-r3_temp_class_parity_arena.rs.txt` as `tests/r3/r3_temp_class_parity_arena.rs` (accessor lines may change if names differ) and add `mod r3_temp_class_parity_arena;` to `tests/r3/main.rs`
- Modify: `CHANGELOG.md`

**Interfaces:**
- Consumes: `pd_solve::ConeArena` from `ConeOutcome.pd_solution` (Task 2).
- Produces (public, read by Task 4 and the A5 oracle):

```rust
// src/engine/l5/snapshot.rs
pub type StateIx = usize;
/// Index into `CapabilitySnapshot::typed_edges` AFTER its sort and filter.
pub type EdgeIx = usize;
/// Stable routine id (the snapshot's routine identity).
pub type RoutineId = String;

#[derive(Debug, Clone, Default)]
pub struct PdArena {
    pub states: Vec<StateNode>,
    /// Each state's incoming links, contiguous: `(callee state, edge)`, edge order.
    pub links: Vec<(StateIx, EdgeIx)>,
    /// Stable routine id → its states (seed included).
    pub by_routine: std::collections::HashMap<RoutineId, Vec<StateIx>>,
}

#[derive(Debug, Clone)]
pub struct StateNode {
    /// The producer's `operation_id` (A5 Compare 1 reads this name).
    pub producer_operation_id: String,
    /// Index into `CapabilitySnapshot::capability_facts` of the producer's
    /// owner-direct fact (the seed's fact; its `extra` carries the raw PD).
    pub producer_fact: usize,
    /// `t` (A5 Compare 1 reads this name).
    pub temp_state: crate::engine::l4::effect_lattice::TempStateKind,
    pub routine: RoutineId,
    /// Guard requirements in `routine`'s frame: `(param index, value)`.
    pub reqs: Vec<(u32, bool)>,
    pub seed: bool,
    pub dist: usize,
    /// Range into `PdArena::links`.
    pub links: std::ops::Range<usize>,
}

pub struct CapabilitySnapshot {
    // ...existing fields...
    /// #69 D2. Serde-skipped: the serialized snapshot moves only where fact
    /// values move.
    #[serde(skip)]
    pub pd_solution: PdArena,
}
```

Field names `producer_operation_id`, `temp_state`, `by_routine` keyed by stable id, and `states[ix]` indexing are what `tests/r3/r3_temp_class_parity_arena.rs` already uses; keep them.

**Acceptance rows greened:** A5 Compare 1.

- [ ] **Step 1: Restore the arena test (red: does not compile).** Restore `.agent/issue-69/parked/task3-r3_temp_class_parity_arena.rs.txt` as `tests/r3/r3_temp_class_parity_arena.rs` and add `mod r3_temp_class_parity_arena;` to `tests/r3/main.rs` (after `mod r3_temp_class_parity;`).

Run: `cargo test -p al-sem --test r3 r3_temp_class_parity_arena` → FAIL `E0609: no field pd_solution`.

- [ ] **Step 2: Map the arena at snapshot build.** In `compose_snapshot`, after `typed_edges` is derived (sorted and filtered), build `HashMap<(from, to, kind, callsite, event), EdgeIx>` over the snapshot edges (use the same accessors `derive_typed_edges` writes). For each `ConeLink`, look up its edge by the same identity; a miss is `debug_assert!(false, "pd arena: cone edge {..} has no snapshot edge")` and increments a census counter `PD_EDGE_UNMAPPED` (release builds drop that link). Map each internal routine id to its stable id with the existing internal→stable map the snapshot uses for facts (`stable_sub_id`). `producer_fact`: the index of the owner's direct fact in `capability_facts` whose `witness_operation_id` equals the producer's op (build a `HashMap<&str, usize>` once). Convert `TempState` with `TempState::to_kind()` (`summary.rs:44`).

- [ ] **Step 3: Census counters (D1.4).** In `cones_census` add `PD_PRODUCERS`, `PD_STATES`, `PD_LINKS`, `PD_ENTRIES`, `PD_SOLVE_BYTES`, `PD_EDGE_UNMAPPED` (`AtomicU64`, added with `add_gated`). Count them at the end of `pd_solve::solve` (producers, states, links, resolved entries) and add `PD_SOLVE_BYTES` = arena bytes (`size_of` of each vec's elements × len + `routine` string bytes + reqs) + `ResolvedByRoutine` bytes, counting each shared rep `Arc<CapabilityFact>` ONCE (`size_of::<CapabilityFact>() + cone_census::capability_fact_heap_bytes(rep)` per distinct `(producer, t)`). Print them in `cones_census::report()` on one line: `[CONES_CENSUS] pd_solve producers=.. states=.. links=.. entries=.. solve_bytes=.. edge_unmapped=..`.

- [ ] **Step 4: Run Compare 1.**

Run: `cargo test -p al-sem --test r3 r3_temp_class_parity_arena` → PASS. If the test's ASSUMED accessor lines (`state.producer_operation_id`, `state.temp_state`) do not match, change the struct, not the test (the struct above is defined to match).

- [ ] **Step 5: Discrimination proof (A5).** In `pd_solve::solve`, skip transitions across intra-SCC edges → red `Compare 1: RecUi2 ∅ != L4 {nt}` (the line also carries `(op ...)`). Revert, green.

- [ ] **Step 6: Goldens.** `pd_solution` is serde-skipped, so NOTHING should move. If a golden moves, the arena leaked into serialization: fix, do not regen. (`scripts/check-goldens` without regen must be green.)

- [ ] **Step 7: CHANGELOG.** `### Added`: "`CapabilitySnapshot.pd_solution` (not serialized): the producer solve as an arena of states and links over the snapshot's typed edges (#69 D2). `ALSEM_CONES_CENSUS=1` reports the solve's producers, states, links, entries and bytes." `### Added`: "A5 oracle: cone/L4 temp-class parity on a matched graph, per producer and reduced (`tests/r3/r3_temp_class_parity*.rs`)."

- [ ] **Step 8: Gate and commit.**

```bash
git add src/engine/l5/snapshot.rs src/engine/l4/capability_cone.rs src/engine/l4/pd_solve.rs \
  src/engine/l5/detector_context.rs tests/r3/main.rs tests/r3/r3_temp_class_parity_arena.rs CHANGELOG.md
git commit -m "#69 D2: snapshot carries the producer solve arena; solve census counters; A5 per-producer parity"
```

---

### Task 4: D3 — witness proofs, one carrier predicate, path identity, incompleteness, effect identity

**Files:**
- Modify: `src/engine/l5/digest.rs`:
  - `FingerprintIndexes` (`:161`) / `build_fingerprint_indexes` (`:189`): hold `&PdArena` and an `edge_ix` lookup (`&SnapshotGraphEdge` → `EdgeIx`, by pointer offset into `snap.typed_edges`).
  - new `fn is_bfs_carrier(node: &str, d: &Fact, f: &Fact) -> bool` and new `fn enumerate_proofs(...)`.
  - `reconstruct_witness_paths` (`:860`): proofs first, then the BFS; combined-set empty diagnostics; terminal-inclusive path identity.
  - `valid_nodes` seed (`:1031-1100`) and the terminal test (`:1170-1215`): both call `is_bfs_carrier`.
  - `merge_normalize_via_paths` (`:2665`): least-restrictive conditionality on equal projected paths.
  - insert branch (`:2979-3010`): go through `merge_normalize_via_paths`.
  - merge (`:2964`) and insert sites: OR `outcome.incomplete` into `had_truncation`.
  - `dedupe_key` (`:2199`): `|kt` / `|nt` suffix. `build_canonical_key` (`:2300`) call site: `|kt` for kt DB effects.
  - `build_fingerprint_indexes_pub` / `pub_idx_as_private` / `reconstruct_witness_paths_pub` (`:3929-4039`): same predicate and arena.
  - the stale guard comments (`~:1040-1100`, `~:1137-1215`): rewrite to the new precondition.
  - existing unit tests near `:4692-4870` that pin first-cond merge: update to least-restrictive.
- Modify: `src/engine/l5/conditionality.rs` (make `rank` `pub(crate)` if it is private)
- Modify: `tests/r3/r3a3_oracles.rs` (`oracle_r3a3_inherited_keys_trace_to_a_direct_producer`, `:221`)
- Restore (parked): `task4-cli_digest_temp_state.rs.txt` into `tests/cli/cli_digest_temp_state.rs` (`line_of`, the A2 helpers, A2a/A2d/A2f/A2g); do this at Step 2
- Modify: `CHANGELOG.md`

**Interfaces:**
- Consumes: `CapabilitySnapshot.pd_solution: PdArena`, `StateNode` (Task 3).
- Produces:

```rust
/// THE carrier predicate for the BFS terminal test, the `valid_nodes` seed, and
/// fingerprint. Ordinary direct facts only: same class, `fact_equivalent`, and
/// `d` is not a raw producer (its temp state is not parameter-dependent).
/// Arena states are NEVER BFS carriers (below-root states are unsound; the
/// root's own states are proofs, not BFS terminals).
fn is_bfs_carrier(_node: &str, d: &Fact, f: &Fact) -> bool {
    !is_pd_snap(d) && is_known_temp_snap(d) == is_known_temp_snap(f) && fact_equivalent(d, f)
}

/// Depth-0 proofs for inherited fact `f` at root `root`: for every NON-seed
/// state s in `by_routine[root]` with `fact_equivalent(rep(s), f)` and
/// `class(s.temp_state) == class(f)`, every link-path from s down to the
/// seed whose FIRST edge's callsite is `f.witness_callsite_id`. Breadth-first
/// by length, ties by the sequence of edge sort keys; a path never revisits a
/// STATE (it may revisit a routine). Returns (paths as edge-index sequences +
/// terminal = producer op, truncated, incomplete, diagnostics).
fn enumerate_proofs(root: &str, f: &Fact, idx: &FingerprintIndexes, cap: usize) -> ProofOutcome;

struct ProofOutcome {
    /// Each: the EdgeIx sequence root→…→owner, and the producer's snapshot fact index.
    paths: Vec<(Vec<EdgeIx>, usize)>,
    truncated: bool,
    incomplete: bool,
    diagnostics: Vec<(String, Option<String>)>,
}
```

`rep(s)` is `snap.capability_facts[s.producer_fact]` read with `s.temp_state` as its class; `fact_equivalent` ignores temp state, so the test is `fact_equivalent(&snap.capability_facts[s.producer_fact], f) && class(&s.temp_state) == is_known_temp_snap(f)` where `class(Known(true)) = kt`, everything else nt. `is_pd_snap(d)` reads `d`'s table `temp_state.kind == "parameter-dependent"`.

**Acceptance rows greened:** A2a, A2d, A2f, A2g.

- [ ] **Step 1: Unit tests (red).** In `digest.rs` `mod tests`, with a literal snapshot (follow the existing witness unit tests' fixture builders):
  - `seed_state_is_not_a_proof_carrier` (Review Focus 5): an arena with only the seed state at root R; `enumerate_proofs(R, f)` returns no path.
  - `proof_never_revisits_a_state`: a two-state cycle `(PermUi, PD0) ↔ (PermUi, PD1)` above the seed; every returned path has distinct states; a path entering PermUi twice exists.
  - `projected_twin_merge_keeps_least_restrictive`: `merge_normalize_via_paths(&[], &[], false, &[p, p], &[CONDITIONAL, UNCONDITIONAL], false, 3)` returns one path with `UNCONDITIONAL`.

Run: `cargo test -p al-sem --lib digest::tests::seed_state_is_not_a_proof_carrier digest::tests::proof_never_revisits_a_state digest::tests::projected_twin_merge_keeps_least_restrictive` → FAIL.

- [ ] **Step 2: Restore the Task-4 parked tests and confirm the four cli rows are red.** Paste `.agent/issue-69/parked/task4-cli_digest_temp_state.rs.txt` into `tests/cli/cli_digest_temp_state.rs` as its header says.

Run: `cargo test -p al-sem --test cli cli_digest_temp_state::a2`
Expected reds include `HelperUiTemp: evidence unavailable`, `ViaCondTwiceTemp: expected 2 viaPaths, got 0`, `DeepRoot: factIds equal`, `IncRoot: viaPathsTruncated false`.

- [ ] **Step 3: `is_bfs_carrier`.** Replace the class+equivalence test at the `valid_nodes` seed and at the terminal check with `is_bfs_carrier(d.subject, d, fact)` / `is_bfs_carrier(routine, d, fact)`. Exclude raw producers in BOTH (the predicate does it). Also include `is_pd_snap` in `ValidNodesKey` only if `ValidNodesKey::of` would otherwise let two facts with different carrier sets share a key (it reads only `fact`; the predicate's `f`-dependence is class + equivalence, already in the key — verify and note in the ledger).

- [ ] **Step 4: Proofs + combined set.** In `reconstruct_witness_paths` Case C, after `first_edges` is computed and BEFORE the empty-`first_edges` early return:
  1. `let proofs = enumerate_proofs(root_id, fact, idx, cap);`
  2. Run the BFS as today, but it no longer pushes `terminal-not-found` itself; it reports `found.is_empty()`.
  3. `first-hop-not-found` (no matching first edge) and `terminal-not-found` (BFS found nothing) are emitted ONLY when `proofs.paths` is empty too. Neither sets `incomplete` any more for the combined decision when proofs exist; when both are empty, keep today's `incomplete = true` + diagnostic (the outcome already yields `unknown` conditionality).
  4. Materialize proof paths: hops = `edge_to_hop(&snap.typed_edges[e], idx)` for each `EdgeIx`, then the terminal `terminal_hop_from_fact(&snap.capability_facts[producer_fact], idx)`.
  5. Raw path identity = full edge sequence + terminal evidence kind + terminal id. Dedup the combined list by that identity, sort by (length, edge sort keys, terminal id), THEN cap at `cap` (`HARD_PATH_CAP`), setting `truncated` if cut.
  6. `incomplete` = BFS `state-limit-exceeded`/`depth-exceeded` OR proof `MAX_DEPTH`/`MAX_STATES` exceeded (same diagnostic strings).

- [ ] **Step 5: Merge + insert normalization.** In `merge_normalize_via_paths`, when a new projected path equals an existing one, keep the LEAST restrictive cond by `conditionality::rank` (lower rank wins; `UNKNOWN` never replaces a ranked cond). Replace the insert branch's `had_truncation = outcome.truncated || projected_paths.len() > MAX_PATHS` and raw stores with:

```rust
let (via, had_truncation, all_paths, all_path_conds) = merge_normalize_via_paths(
    &[], &[], false, &projected_paths, &path_conds,
    outcome.truncated || outcome.incomplete, MAX_PATHS,
);
```

and at the merge site pass `outcome.truncated || outcome.incomplete` as `new_truncated`. Update the existing unit tests near `:4692-4870` whose expectations encoded first-cond-wins; each changed expectation gets a one-line comment `// #69 D3: least-restrictive on projected twins`.

- [ ] **Step 6: Identity.** `dedupe_key`: append `|kt` when `is_known_temp_snap(fact)`, else `|nt` (DB effects and non-DB alike; non-DB facts are always nt). At the `build_canonical_key` call site, when the effect type starts with `DB_` and the fact is kt, pass `effect_type` as `format!("{effect_type}|kt")` into the key's last component (nt unchanged, so nt canonical keys do not move).

- [ ] **Step 7: Fingerprint consumers.** `pub_idx_as_private` and `build_fingerprint_indexes_pub` carry the arena; `reconstruct_witness_paths_pub` calls the same `reconstruct_witness_paths`, so it inherits proofs and `is_bfs_carrier`. Grep `fingerprint_query.rs:577, 598` callers compile unchanged.

- [ ] **Step 8: Oracle.** Rewrite `oracle_r3a3_inherited_keys_trace_to_a_direct_producer`: drop the PD→kt allowance; instead, for each fixture also `compose_snapshot` the model and accept an inherited key `k` at routine R iff `k` is a direct key in the fixture OR R has a non-seed `pd_solution` state whose producer fact's `inherited_fact_key` base (class suffix stripped) + the state's class suffix equals `k`. Update its comment to the new precondition. Rewrite the two stale guard comments in `digest.rs` to: "every inherited fact has a same-class ORDINARY producer reachable below the root, or a same-class non-seed arena state AT the root (a proof); arena states below the root are never carriers."

- [ ] **Step 9: Run.**

Run: `cargo test -p al-sem --lib digest::tests` → PASS.
Run: `cargo test -p al-sem --test cli cli_digest_temp_state::` → all compiled rows PASS (the Task-5 rows are still parked).
Run: `cargo test -p al-sem --test r3 r3a3_oracles` → PASS.

- [ ] **Step 10: Discrimination proofs.**
  - A2a: make `enumerate_proofs` return no paths (early `return` of an empty outcome) → `HelperUiTemp: evidence unavailable`.
  - A2d (1): keep only the first link per state in proof enumeration → `ViaCondTwiceTemp: expected 2 viaPaths`.
  - A2d (2): dedup raw paths by edge sequence only (drop terminal from identity) → `HelperTwoTemp: conditionality conditional`.
  - A2d (3): keep the first cond on an equal projected merge → `HelperTwoTemp: conditionality conditional`.
  - A2d (4): restore the insert branch's raw store (skip `merge_normalize_via_paths`) → `HelperTwoTemp: expected 1 viaPath, got 2`.
  - A2d (5): fold the BFS's own `terminal-not-found` into `had_truncation` (set `incomplete = true` when the BFS found nothing, as before) → `CondOnlyTemp: conditionality unknown`.
  - A2g: drop the `|| outcome.incomplete` at the insert site → `IncRoot: conditionality conditional`.
  - A2f: drop the `|kt` canonical suffix → `DeepRoot: factIds equal`.

- [ ] **Step 11: Goldens.** Expected causes: D3 witnesses (kt facts gain proof paths and source evidence; `evidence-unavailable` drops), projected-twin dedup (fewer `viaPaths`, `viaPathsTruncated` flips false where only twins exceeded `MAX_PATHS`), `viaPaths` ORDER re-sort through the shared helper and the `canonicalKey`/`factId` move that follows (R6-F01), the `|kt` canonical suffix on kt DB effects, `dedupe_key` splits, incomplete → `viaPathsTruncated: true` and `conditional` → `unknown` (the widened meaning), d47/d49/d51 ordering evidence. Families: `r4f`, `cli-b`, `cli-a`, `cli-c*`, `gate`, `r4`.

- [ ] **Step 12: CHANGELOG.** `### Fixed`: "Digest witnesses for writes a call site makes temporary keep their evidence, conditionality and every via-path through the fact's first hop (#69 D3): proofs come from the root's own solve states; the BFS and fingerprint share one carrier predicate." "`viaPaths` are deduplicated by projected path on an effect's first fact too; equal projected paths keep the least restrictive conditionality." "A kt and an nt DB effect never share an occurrence id (`|kt` canonical suffix; class in the dedupe key)." `### Changed`: "`viaPathsTruncated` now also means a non-empty path set is not known to be complete (state or depth limit, BFS or proof enumeration); such effects can move from `conditional` to `unknown` conditionality and ordering chains report `capped`."

- [ ] **Step 13: Gate and commit.**

```bash
git add src/engine/l5/digest.rs src/engine/l5/conditionality.rs tests/r3/r3a3_oracles.rs \
  tests/cli/cli_digest_temp_state.rs CHANGELOG.md <regenerated golden paths>
git commit -m "#69 D3: witness proofs from root solve states; one carrier predicate; terminal-inclusive path identity; incomplete folds into truncation; class in effect identity"
```

---

### Task 5: D4 — per-operation tempState join, incoherence diagnostic, schema bump; the cross-cutting proofs

**Files:**
- Modify: `src/engine/l5/digest_cli.rs`: `TempJoin`, `JoinProblem`, `join_temp_states`, `TempJoin::render`; `for_effect` (`:621`) uses them; `EntryDiagnostic` (`:565`) gains `fact_id`, `operation_id`, `reason`; its serializer (`~:1037-1052`); `schemaVersion` `1.6.0` (`:1259`); re-paste the parked A4b tests.
- Modify: `src/engine/l5/prove.rs` (`schemaVersion` `1.4.0` at `:725`; shared builder `:959-975` serializes the new optional fields)
- Modify: `src/engine/l4/effect_lattice.rs` (`TempStateKind`: add `PartialOrd, Ord`)
- Restore (parked): `task5-digest_cli_tests.rs.txt` (Step 1) and `task5-cli_digest_temp_state.rs.txt` into `tests/cli/cli_digest_temp_state.rs` (Step 4)
- Modify: `CHANGELOG.md`

**Interfaces:**
- Consumes: the effect's cone fact class (`is_known_temp_snap`), L4 rows (`bundle.db_effects(rix)` with `operation_id`, `temp_state`).
- Produces:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JoinProblem { NoL4Row, ClassAbsent }

#[derive(Debug, Clone, Default)]
pub struct TempJoin {
    pub states: std::collections::BTreeSet<TempStateKind>,
    pub problems: Vec<(String /* op */, JoinProblem)>,
}

impl TempJoin {
    /// The `detail.tempState` field: `None` (absent) when any op has `NoL4Row`
    /// or no state was joined; else the sorted distinct states rendered with
    /// `temp_state_str` and joined with `,` (today's format).
    pub fn render(&self) -> Option<String> {
        if self.problems.iter().any(|(_, p)| *p == JoinProblem::NoL4Row) || self.states.is_empty() {
            return None;
        }
        Some(self.states.iter().map(temp_state_str).collect::<Vec<_>>().join(","))
    }
}

/// D4: per originating op — no row → NoL4Row; rows but none in the effect's
/// class → ClassAbsent and ALL its rows; else only its in-class rows.
pub fn join_temp_states(
    effect_is_kt: bool,
    op_ids: &[String],
    rows: &[(&str, TempStateKind)],
) -> TempJoin {
    let mut j = TempJoin::default();
    for op in op_ids {
        let mine: Vec<&TempStateKind> =
            rows.iter().filter(|(o, _)| *o == op).map(|(_, t)| t).collect();
        if mine.is_empty() {
            j.problems.push((op.clone(), JoinProblem::NoL4Row));
            continue;
        }
        let in_class: Vec<&TempStateKind> = mine
            .iter()
            .copied()
            .filter(|t| matches!(t, TempStateKind::Known(true)) == effect_is_kt)
            .collect();
        if in_class.is_empty() {
            j.problems.push((op.clone(), JoinProblem::ClassAbsent));
            j.states.extend(mine.into_iter().cloned());
        } else {
            j.states.extend(in_class.into_iter().cloned());
        }
    }
    j
}

pub struct EntryDiagnostic {
    pub kind: &'static str,
    pub effect_type: Option<String>,
    pub fact_subject: Option<String>,
    pub fact_id: Option<String>,
    pub operation_id: Option<String>,
    /// `no-l4-row` | `class-absent`
    pub reason: Option<&'static str>,
}
```

Check the current `temp_state_str` join separator in `for_effect` and keep it (the A4/A2b tests read `known(false),known(true)`, so `,`). `for_effect` returns `TempJoin` (or `(Option<String>, Vec<problems>)`); the caller sets `detail.tempState = join.render()` and pushes one `EntryDiagnostic { kind: "temp-state-incoherent", fact_id: Some(effect factId), operation_id: Some(op), reason: Some(..), .. }` per problem. Existing `EntryDiagnostic` literals get `fact_id: None, operation_id: None, reason: None`. The serializer emits `factId`, `operationId`, `reason` only when `Some` (additive).

**Acceptance rows greened:** A4b, A1a, A2b, A2c, A2e (and A1b's proof).

- [ ] **Step 1: Re-paste A4b (red).** Paste `.agent/issue-69/parked/task5-digest_cli_tests.rs.txt` (below its header) back at the end of `digest_cli.rs` `mod tests`.

Run: `cargo test -p al-sem --lib digest_cli::tests::temp_join` → FAIL (does not compile: `join_temp_states`, `JoinProblem`, `TempStateKind: Ord`).

- [ ] **Step 2: Implement** the types and `join_temp_states` above; derive `PartialOrd, Ord` on `TempStateKind` (variant order Known < ParameterDependent < Unknown; `BTreeSet` order only drives the rendered order, which `render` already sorts by the rendered string if today's output is sorted by string — keep today's order: if `for_effect` sorted by `temp_state_str` string, render by collecting strings into a `BTreeSet<String>`).

Run: `cargo test -p al-sem --lib digest_cli::tests::temp_join` → PASS.

- [ ] **Step 3: Wire it.** `for_effect(root_stable, op_ids, effect_is_kt)` builds `rows` from `self.bundle.db_effects(rix)` and calls `join_temp_states`. `effect_is_kt` = the effect's cone fact class (`acc.temp_state` known(true)). Add the diagnostics. Bump digest to `1.6.0` and prove to `1.4.0`.

- [ ] **Step 4: Run the remaining rows.** Restore `.agent/issue-69/parked/task5-cli_digest_temp_state.rs.txt` into `tests/cli/cli_digest_temp_state.rs` as its header says.

Run: `cargo test -p al-sem --test cli cli_digest_temp_state::` → all PASS; nothing remains parked.
Run: `cargo test -p al-sem --test r3 r3_temp_class_parity` → PASS.

If a row is still red, read its quoted lines: a D1/D3 defect is fixed here (this task is the first where those claims are checkable); record the fix in the ledger with the line it greened.

- [ ] **Step 5: Discrimination proofs (all rows that needed D4 to be checkable).** Each: Edit, `grep -c` = 1, run the row's test, quote the named red, revert, green.
  - A4b: in `join_temp_states`, union all ops' rows before checking classes (compute `in_class` over every row of every op) → `ClassAbsent not reported`.
  - A1a: `temp_state: None` in `OrderingEffectInput` (`digest.rs:~3136`) → `RecUiTemp: WRITE_PENDING_AT_UI on a known(true) write`.
  - A1b: put producers back into `fact_cone_for_scc` for recursive SCCs (feed the recursive SCC's producers into its cone `direct`) → `RecUiTemp: temp-state-incoherent class-absent`.
  - A2b (i): drop the class from `dedupe_key` → `HelperUiBoth: expected 2 DB_INSERT, got 1`.
  - A2b (ii): drop D4's class filter (contribute all rows of every op) → `HelperUiBoth: tempState known(false),known(true)`.
  - A2c: make the depth-0 class test in `enumerate_proofs` class-agnostic → `HelperUiBothDeep: kt path contains Row callsite`.
  - A2e, one per claim:
    1. re-merge producers into ordinary keys (put them back into `direct` in `compose_cone_over_graph`) → `MixedUiTemp: label missing`;
    2. collapse a producer's states to "any nt ⇒ all nt" (in the solve, when a `(p, r)` has any nt state, drop its kt states) → `TwoRecMix: no known(true)`;
    3. in `cross_edge`, map `Known(true)` to `Unknown` on intra-SCC edges → `CycA: label present`;
    4. skip ordinary facts of recursive members in `fact_cone_for_scc` → `CycQTemp: label missing`;
    5. drop use (a) for the routine's own entries (skip `subject`'s `ResolvedEntry`s in `inherited_facts_by_bfs`) → `RecH: no DB_INSERT`;
    6. restore sibling producers in `inherited_facts_by_bfs` → `SibP2: label present`;
    7. skip use (a) for root-SCC members → `RootCyc1: no DB_INSERT`;
    8. cap proof enumeration at revisit-free ROUTINE paths → `PermMix: nt evidence unavailable`;
    9. drop `edge_reqs` in `cross_edge` (pass `&[]`) → `GuardFwdFalse: DB_INSERT present`;
    10. drop `reqs` on states inside recursive SCCs → `GCycFalse: DB_INSERT present`;
    11. stop propagation at free states (non-PD `t`, empty `reqs`) and hand them to the shared cone (the rev-3 rule) → `GTFalse: DB_INSERT present`;
    12. admit arena states below the root in `is_bfs_carrier` → `ForwardMix: nt viaPaths != [ForwardMix→Both→RealWriter]`;
    13. skip the BFS after the proofs → `MixedCarrier: expected 2 viaPaths`.

- [ ] **Step 6: Goldens.** Expected causes: D4 values (`tempState` filtered to the effect's class; a `NoL4Row` op makes it absent), new `temp-state-incoherent` entry diagnostics, digest `schemaVersion` `1.6.0`, prove `1.4.0` (`cli-a`, `cli-b`). Any `temp-state-incoherent` item in a golden is a residual cone/L4 drift: quote it in the receipt with its op and reason and explain it from the fixture's source, or fix the code.

- [ ] **Step 7: CHANGELOG.** `### Changed`: "digest/prove `detail.tempState` is a per-operation join filtered to the effect's class; an op with no L4 row makes the field absent (#69 D4). New entry diagnostic `temp-state-incoherent` with `factId`, `operationId` and `reason` (`no-l4-row` | `class-absent`). digest `schemaVersion` 1.6.0, prove 1.4.0 (additive)." `### Fixed`: "The ordering engine's physical-write filter now grades a write the way `detail.tempState` reports it (#69)."

- [ ] **Step 8: Gate and commit.**

```bash
git add src/engine/l5/digest_cli.rs src/engine/l5/prove.rs src/engine/l4/effect_lattice.rs \
  tests/cli/cli_digest_temp_state.rs CHANGELOG.md <regenerated golden paths>
git commit -m "#69 D4: per-operation class-filtered tempState join; temp-state-incoherent diagnostic; digest 1.6.0 / prove 1.4.0"
```

---

### Task 6: Branch measurement — suite diff, CDO, cost budget; final CHANGELOG numbers

No new source. This task produces evidence; it commits only `CHANGELOG.md` (measured numbers). If the cost budget fails, it does NOT commit: the merge is blocked and the spec returns to the panel (D1.4).

**Files:**
- Modify: `CHANGELOG.md` (measured solve cost and CDO residuals)
- Write (not committed): `.agent/issue-69/ledger.md` sections "## Gate results", "## Measurements"

- [ ] **Step 1: Suite diff B0 vs branch.** Create a B0 worktree `git worktree add U:/Git/al-sem-issue-69-b0 049b166c` (check `df -h /u` first; ≥ 40 GB free). Run `cargo test -p al-sem --no-fail-fast > b0.log 2>&1` there and on the branch (both with `CDO_WS` unset), extract `test <name> ... ok|FAILED|ignored` lines, diff. Every outcome change must be one of the #69 tests or a test this branch edited; anything else is a regression to fix (in the owning task's code, as a new commit on this branch with its own gate).

- [ ] **Step 2: Branch gate.** `scripts/ci-steps all > .agent/issue-69/ci-all.log 2>&1; echo rc=$?` and `scripts/check-goldens --verify-coverage` → rc=0 both.

- [ ] **Step 3: CDO gate.** `scripts/cdo-gate U:/Git/DO-cdo-baseline/Cloud > .agent/issue-69/cdo-gate.log 2>&1; echo rc=$?` → rc=0. `target/cdo/aldump.exe --program-call-graph-stats U:/Git/DO-cdo-baseline/Cloud` must be byte-identical to B0's (build B0's with `--profile cdo --bin aldump` in the B0 worktree; `cmp` the two JSON outputs).

- [ ] **Step 4: Detector deltas on CDO.** `alsem analyze` (cross-app default), `--scope primary` and `--scope all`, B0 and branch (`release-fast`). Per detector (full ids): added, removed, changed, cap-substituted findings. Triage each against source (`triage-findings` skill); record in the ledger. A change not explained by D1/D3/D5 (spec "False-positive and failure shapes") is a bug.

- [ ] **Step 5: `temp-state-incoherent` count on CDO.** Run `alsem digest U:/Git/DO-cdo-baseline/Cloud --deterministic --format json --file <f> ...` over every `.al` file of the workspace in batches of 200 `--file` arguments (a driver script in the scratchpad, not committed). Count `entryDiagnostics` items with `kind == "temp-state-incoherent"` by `reason`, and the total DB_* effects checked. Target 0; explain every residual from source in the ledger.

- [ ] **Step 6: Cost budget (D1.4), DO and CDO.** `release-fast` builds of `alsem` for B0 and branch. Per workspace (DO = `U:/Git/DO/Cloud` at one recorded SHA for both sides; CDO pinned), 3 runs each side of:
  - `python scripts/peak_rss.py <alsem> analyze <ws>` → `elapsed_s`, `peak_rss_mb`;
  - the same with `ALSEM_CONES_CENSUS=1` once per side for the counters (branch: the `pd_solve` line; record `solve_bytes`);
  - the Step 5 digest batch driver under `peak_rss.py` (one batch run is the "digest probe").
  Budget, each against B0 same workspace same profile: `solve_bytes ≤ 5% × B0 median peak RSS`; branch peak RSS within B0's noise band (min..max of B0's 3 runs, widened by nothing); branch wall-time median ≤ B0 median × 1.10. Record every number and the pass/fail per line in the ledger. Any FAIL → stop, report, do not commit.

- [ ] **Step 7: CHANGELOG numbers and commit.** Add to the #69 `### Fixed` entry: the measured solve cost line (producers/states/bytes on DO and CDO, wall/RSS deltas) and the CDO `temp-state-incoherent` residual count. Run `scripts/ci-steps task` (log, rc=0), then:

```bash
git add CHANGELOG.md
git commit -m "#69: CHANGELOG — measured solve cost and CDO coherence residuals"
```

Remove the B0 worktree afterwards (`git worktree remove U:/Git/al-sem-issue-69-b0`; it was created from a commit with no submodule, so plain remove works) and run `scripts/sweep-builds` if disk is low.
