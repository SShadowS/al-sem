# Issue #107: Event Publisher Roles Compose Subscribers Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** An event publisher's `RecordRoleSummary` for a record parameter includes what its subscribers do to that parameter, so every routine that raises the event (and every role-reading detector) sees it through the existing call-site composition.

**Architecture:** The model `EventEdge` / `CombinedEdge` carry dispatch metadata (`conditional`, `isolated`). A new `event_dispatch` module turns a routine's `event-dispatch` edges into per-parameter dispatch bindings once per solve (prologue). The roles step folds the bindings' presence facts (the c1b fold, factored out, with `optional_load` on the load trio of optional bindings) and the walker applies the dispatch to every exit state by exact enumeration (subsets of optional bindings x orders), falling back to the opaque transfer for unseen or isolated dispatch. A capped roles SCC publishes the opaque role.

**Tech Stack:** Rust (`al-sem` crate), cargo tests (`--lib`, umbrella `--test gap`), golden families via `scripts/check-goldens`.

**Spec:** `docs/superpowers/specs/2026-10-19-issue-107-publisher-roles-subscribers-design.md` (Design 1-7). Also `.agent/issue-107/ledger.md`: "Plan notes" and the two step-6 Rulings (overload collision; `with_roles_iteration_cap`). Acceptance tests already written and red: `U:/Git/al-call-hierarchy/.agent/runs/20261019-071107-16946d/acceptance-tests.md`.

## Global Constraints

- `rustfmt <file>` per file (edition 2024), never `cargo fmt`. Edit files with the Edit/Write tools only, never a script (discrimination breaks too: break with Edit, revert with Edit).
- No `#[allow(` anywhere new. Where a new function would pass clippy's 7-argument limit, bundle the read-only lookups in `cfg_walker::CalleeLookup` (Task 5) instead.
- Never touch `.github/ scripts/ .claude/ CLAUDE.md .gitignore`, `Cargo.toml` version fields, `tree-sitter-al`, or commit anything under `.agent/` (the receipt `.agent/golden-triage.md` and the parked tests in `.agent/issue-107/pending-tests/` are written, never committed).
- `export TREE_SITTER_AL_PATH=U:/Git/al-call-hierarchy/tree-sitter-al` for every cargo/script command; run from `U:/Git/al-sem-issue-107-a1`. Never pipe a gate through `| tail`: redirect to a log and `grep` it.
- Every new or changed test gets a discrimination proof (`discrimination-proof` skill): break the code with Edit, run, see the named test fail, revert, see it pass; record both outcomes in the task report. A break that stays green is evidence about the test: say so, never hide it.
- Goldens move only as the spec's Design 7 "Every output that can move" says (bodied publishers with a subscriber that touches a record parameter by name, their transitive callers, `parameterRoles` in r3a2/r3a5 goldens and manifests, d37/d39/d41/d42 (default) and d40 (opt-in) findings at such calls). `d3-missing-setloadfields`, every non-role detector, call edges, the histogram, db-effect rows, the event graph and its r2c projection do not move. Before any regen: run `scripts/check-goldens` without regen, triage the failures with the `golden-diff-triager` agent, write `.agent/golden-triage.md` (every `MOVED:` path), then `scripts/check-goldens --regen`, then `scripts/check-goldens` green. An unexplained line: stop, find the root cause.
- Acceptance tests are committed WITH the code that makes them green. `cargo test -p al-sem --lib` must compile at every commit, so a task commits only once the lib target compiles with exactly the tests it has placed.
- Each task names the parked tests it turns green (the split by presence facts, Task 4, and path facts, Task 5, is read off each row's failing assertions). If a listed row is not green at its task, find out why before moving it to a later task, and say so in the report.
- The commit gate is `scripts/ci-steps task` (log + grep). The pre-commit hook rejects a red golden family.
- CHANGELOG under `## [Unreleased]` (Task 7).
- `EXACT_DISPATCH_MAX = 4`. Bindings per (publisher, parameter) above 4 are an unseen dispatch.

## Review Focus

1. A subscriber that declares `Sender` next to `var Rec` on an `IncludeSender` event must stay an exact binding (the overload-collision rule excludes `Sender`), not opaque. Pinned: Task 4 `review_sender_parameter_keeps_the_binding_exact`.
2. A publisher with two record parameters and a subscriber that declares only the second: the first keeps its base role. Pinned: Task 4 `review_partial_binding_leaves_the_other_parameter_alone`.
3. A resolved subscriber with no body (symbol-only, or bodyless by hand): its binding is visible but opaque (c1b opaque reason), so the publisher's facts are `unknown`, never `no`. Pinned: Task 5 `review_bodyless_subscriber_is_opaque`.
4. Exactly 4 bindings, all optional: exact path with 65 sequences; 5 bindings: unseen. Pinned: Task 5 `dispatch_sequences_cover_every_subset_and_order` (65) and the existing `row12_five_subscribers_are_unseen`.
5. A by-value publisher parameter with an exact `Modify` subscriber: no presence change (the caller side is not `var`), but the entry requirement composes (c1a). Pinned: Task 5 `review_by_value_publisher_param_composes_entry_only`.

---

### Task 1: c1b accumulates `mutates_param` (zero-move claim)

**Files:**
- Modify: `src/engine/l4/summary_runner.rs` (`compose_roles_only`, the `p.mutates_param = …` line, `:728-731`)

**Interfaces:**
- Produces: nothing new. Changes c1b's `mutates_param` to an accumulating join.

- [ ] **Step 1: Park the acceptance tests.** Copy the six files as they are now into `.agent/issue-107/pending-tests/` (keep the relative paths): `tests/gap/gap_issue107_publisher_roles.rs`, `tests/gap/main.rs`, `src/engine/l4/capability_cone.rs`, `src/engine/l4/pd_solve.rs`, `src/engine/l4/summary_runner.rs`, `src/program/resolve/index.rs`. Then `git stash push -u -m "issue-107 a1: parked acceptance tests (copies in .agent/issue-107/pending-tests)" -- <the six paths>`. Verify `git status --short` shows only `.agent/issue-107/` untracked and `cargo test -p al-sem --lib --no-run` compiles. Later tasks Read tests from the copies and place them with Edit; the stash is a safety net, never popped.
- [ ] **Step 2: Implement.** In the c1b loop replace the per-binding overwrite with `p.mutates_param = join_presence(join_presence(p.mutates_param, derived), callee_mutates)`, where `derived = join_presence(join_presence(p.persists_current_record, p.validates_param), p.copies_into_param)` and `callee_mutates` is `cr.mutates_param` in the else branch and `EffectPresence::No` in the opaque branch (which already joins `unknown` through `derived`). Comment: a join, so independent of call order; for every callee today `cr.mutates_param` equals the derivation over facts already joined into `p`, so no call site moves; it carries an unmasked mutation through forwarders (#107 Design 3, R3-1).
- [ ] **Step 3: Zero-move check.** `cargo test -p al-sem --lib > <log> 2>&1` green; `bash scripts/check-goldens > <log> 2>&1` green with zero golden moves (no regen). A moved golden falsifies the spec's "no-op today" claim: stop and report it.
- [ ] **Step 4: Discrimination.** None possible by design (the claim is that no current input observes it). State that in the report; the pinning test is `row5d_prime_mutates_accumulates_in_either_order`, whose split proof is recorded in Task 4.
- [ ] **Step 5: Commit.** `rustfmt src/engine/l4/summary_runner.rs`; `bash scripts/ci-steps task > <log> 2>&1` green; `git add src/engine/l4/summary_runner.rs`; `git commit -m "#107: c1b accumulates mutates_param across call sites (no output moves)"`.

### Task 2: One subscriber -> publisher parameter mapping

**Files:**
- Modify: `src/engine/l4/summary_runner.rs` (next to `event_param_temp_state`, `:906`)
- Modify: `src/engine/l4/pd_solve.rs` (`carry_reqs`, `:170-185`)
- Test: `src/engine/l4/summary_runner.rs` new `#[cfg(test)] mod issue107_pending_tests` (placed from the parked copy: `row13_event_publisher_param_binds_by_name_quotes_ignored` with the helpers it uses: `APP`, `param`, `model`, and only the `use` lines those need); `src/engine/l4/pd_solve.rs` tests (place `carry_reqs_event_dispatch_ignores_quotes_in_parameter_names`)

**Interfaces:**
- Produces: `pub(crate) fn event_publisher_param<'p>(publisher: &'p ModelRoutine, subscriber: &ModelRoutine, sub_param: u32) -> Option<&'p ModelParameter>` in `summary_runner.rs`: by name, case-insensitive, `trim().trim_matches('"')` on both sides; `None` when the subscriber has no parameter `sub_param` or no publisher parameter has that name.

- [ ] **Step 1: Place the two tests; confirm red.** `cargo test -p al-sem --lib -- row13_event_publisher_param carry_reqs_event_dispatch_ignores_quotes` → row13 does not compile (`event_publisher_param` missing). Comment row13 out only long enough to see the `carry_reqs` row fail with `left: Some([]) right: Some([(0, true)])`, then restore it with Edit.
- [ ] **Step 2: Implement** `event_publisher_param`; `event_param_temp_state` calls it (its own lookup goes); `carry_reqs` maps `j` through `event_publisher_param(publisher, subscriber, j).map(|pp| pp.index)` (the inline copy goes). `grep -n "eq_ignore_ascii_case" src/engine/l4/pd_solve.rs src/engine/l4/summary_runner.rs` shows no other subscriber/publisher name match.
- [ ] **Step 3: Green.** Both tests pass; `bash scripts/check-goldens > <log> 2>&1` green with zero moves (Design 1 expects none: source names are already unquoted). A move: triage per Global Constraints before anything else.
- [ ] **Step 4: Discrimination.** (i) drop `trim_matches('"')` in `event_publisher_param` → row13 and the `carry_reqs` row fail; (ii) restore the old inline mapping in `carry_reqs` → only the `carry_reqs` row fails. Revert each.
- [ ] **Step 5: Commit.** rustfmt both files; `scripts/ci-steps task` green; `git commit -m "#107: one subscriber-to-publisher parameter mapping, quotes ignored"`.

### Task 3: Dispatch metadata on the edges

**Files:**
- Modify: `src/program/resolve/index.rs` (`Subscription` `:104-124`, the merge loop `:449-470`)
- Modify: `src/program/model/events.rs` (`EventEdge` `:84-92`, the push `:547`)
- Modify: `src/engine/l4/combined_graph.rs` (`CombinedEdge` `:37-48`, the dispatch push `:367`)
- Modify every other `EventEdge { … }` literal (`conditional: false`): `src/engine/gate/events.rs:634`, `src/engine/gate/format_html.rs:983`, `src/engine/l5/event_flow/tests.rs:36`, `tests/cli/cli_c_events_differential.rs:668,676`; every other `CombinedEdge { … }` literal (`dispatch: None`): `grep -rn "CombinedEdge {" src tests` (combined_graph `:267`, db_effect_solver, effect_query, reverse_index, d17, event_flow/tests, path_walker, l5/test_support, `tests/l4_summary_differential.rs`)
- Test: `src/program/resolve/index.rs` tests (place `issue107_mixed_readings_set_conditional_any`); `issue107_pending_tests` (place `row5c_edge_metadata_marks_mixed_readings_conditional` with `id_of`, `EDGE_SRC`, its `use` lines)

**Interfaces:**
- Produces: `Subscription.conditional_any: bool` (UNION over the merged readings: true when any reading has any condition; `conditions` stays the intersection). `EventEdge.conditional: bool` (= the subscription's `conditional_any`; NOT added to `PEventEdge`). `#[derive(Debug, Clone, PartialEq, Eq, salsa::SalsaValue)] pub struct DispatchMeta { pub conditional: bool, pub isolated: bool }` and `CombinedEdge.dispatch: Option<DispatchMeta>`, `Some` only on `event-dispatch` edges, `isolated = sym.isolated == Some(true)`.

- [ ] **Step 1: Place the two tests; confirm red** (`E0609` on `conditional_any`, `conditional`, `dispatch`).
- [ ] **Step 2: Implement** the three fields. In the merge loop take `conditional_any = !entry.conditions.is_empty()` BEFORE the `retain`, OR `!other.conditions.is_empty()` per other reading.
- [ ] **Step 3: Green.** Both tests pass; `bash scripts/check-goldens > <log> 2>&1` green with zero moves (nothing projects the new fields).
- [ ] **Step 4: Discrimination.** (i) compute `conditional_any` after the `retain` (intersection) → `issue107_mixed_readings_set_conditional_any` fails; (ii) `conditional: false` at the `events.rs` push → row5c fails on `HandleMixRead`; (iii) `isolated: false` in the combined push → row5c fails on `OnIso`. Revert each.
- [ ] **Step 5: Commit.** rustfmt touched files; `scripts/ci-steps task` green; `git commit -m "#107: event edges carry dispatch metadata (conditional, isolated)"`.

### Task 4: Dispatch bindings and the presence fold

**Files:**
- Create: `src/engine/l4/event_dispatch.rs` (register in `src/engine/l4/mod.rs`)
- Modify: `src/engine/l4/summary_runner.rs` (`compose_roles_only`, `member_roles`, `SccComputeCtx` `:1474`, the prologue of `compute_summaries_v2_bundle_with_leaves` near `:1177`)
- Goldens: per Global Constraints
- Test: create `tests/gap/gap_issue107_publisher_roles.rs` from the parked copy with the rows below plus only the helpers/fixtures they use (no dead code under `-D warnings`), and the `mod gap_issue107_publisher_roles;` line in `tests/gap/main.rs`; place both `issue107_*` tests and `write_fixture_107` in `capability_cone.rs` `issue70_publisher_roles_tests`

**Interfaces:**
- Consumes: `event_publisher_param` (Task 2); `CombinedEdge.dispatch` (Task 3).
- Produces, in `event_dispatch.rs`:
  - `pub(crate) const EXACT_DISPATCH_MAX: usize = 4;`
  - `pub(crate) struct ExactBinding { pub subscriber: String, pub q: u32, pub callee_is_var: bool, pub optional: bool, pub path_opaque: bool }`
  - `pub(crate) enum DispatchKind { Unseen { too_many: bool }, Exact(Vec<ExactBinding>) }`
  - `pub(crate) struct ParamDispatch { pub param_index: u32, pub caller_is_var: bool, pub kind: DispatchKind }`
  - `pub(crate) fn dispatch_bindings(routine: &ModelRoutine, edges: &[CombinedEdge], routines_by_id: &HashMap<String, &ModelRoutine>) -> Vec<ParamDispatch>` (empty when the routine has no `event-dispatch` edge or `!routine.body_available`; one entry per record parameter of `routine` with at least one binding, in parameter order)
  - `pub(crate) fn optional_load(v: EffectPresence) -> EffectPresence` (`Yes -> No`, else unchanged)
  - In `summary_runner.rs`: `SccComputeCtx.dispatch_by_id: &'a HashMap<String, Vec<ParamDispatch>>` (only routines with a non-empty result); `fn fold_callee_presence(p: &mut RecordRoleSummary, callee_role: Option<&RecordRoleSummary>, optional: bool)`; `compose_roles_only` gains `dispatch: &[ParamDispatch]`.

`dispatch_bindings` rules (Design 2 + Ruling):
- A subscriber `S` (an edge's `to`) is **Opaque** when any edge to it has `resolution != "resolved"`, or `S` is not in `routines_by_id`, or (overload-collision Ruling) some parameter of `S` whose trimmed, unquoted name is not `sender` (case-insensitive) has `event_publisher_param(routine, S, q) == None`. Any Opaque subscriber makes EVERY record parameter `Unseen { too_many: false }`. (The element parameter of the Ruling belongs to platform events, which have no publisher routine: non-goal.)
- Otherwise, per edge, every `q` of `S` with `event_publisher_param = Some(p)`, `p.is_record`, gives `ExactBinding { subscriber: S, q, callee_is_var: q.is_var, optional: conditional || isolated, path_opaque: isolated }` for `p` (`dispatch` `None` = both false). Dedupe by `(S, q)`, OR-ing the flags. `caller_is_var = p.is_var`.
- More than `EXACT_DISPATCH_MAX` bindings for `p` → `Unseen { too_many: true }`.

Presence fold (Design 3, Design 6), in `compose_roles_only` after the call-site loop and before the walker, and in `member_roles` for a `fixed_roles` member after cloning its base roles (no walker there):
- `fold_callee_presence` is the old c1b loop body (opaque branch, else branch, Task 1's accumulating `mutates_param`) with one change: when `optional`, the trio `loads_from_db_param`, `initialises_param`, `copies_into_param` from `cr` go through `optional_load` before the join. `mutates_param` still joins the UNMASKED `cr.mutates_param` (every role's `mutates_param` covers its own `copies_into_param`, `summary_runner.rs:541` and the fold), which is the spec's unmasked-copies shadow; no separate shadow. The call-site loop calls it with `optional = false`: byte-identical.
- For each `ParamDispatch` with `caller_is_var`: `Unseen` → one `fold_callee_presence(p, None, false)`. `Exact` → per binding with `callee_is_var`: the subscriber's role for `q` from `lookup`, `None` when there is no summary, no role for `q`, or `!body_avail_by_id[subscriber]`; `optional` from the binding.

- [ ] **Step 1: Place the tests; confirm red.** Gap rows (they turn green here): `row3_two_subscribers_every_order_persists`, `row5_manual_loader_is_masked`, `row5b_manual_loader_does_not_destroy_the_callers_load`, `row5c_mixed_readings_mask_the_load`, `row5d_prime_mutates_accumulates_in_either_order`, `plan_manual_copy_then_opaque_helper_keeps_the_mutation`, `row6b_isolated_validate_error`, `row10_cycle_converges_with_the_subscribers_effects`. Lib: `issue107_dependency_publisher_presence_composes_subscribers` (red) and `issue107_symbol_only_dependency_publisher_roles_unchanged` (green by design: it pins Design 6's skip). New gap tests, written now:

```rust
/// Ruling (step 6): two overloads share one event id, so `OnPair(var Rec)`
/// also gets a RESOLVED edge to `HandlePairQty(var Rec; Qty)`, whose `Qty`
/// binds no parameter of that overload: opaque there, exact on the other.
#[test]
fn row7d_overload_collision_resolved_edge_is_opaque() {
    // codeunit 50100 Pub: [IntegrationEvent(false, false)] OnPair(var Rec: Record Cust)
    //   and [IntegrationEvent(false, false)] OnPair(var Rec: Record Cust; Qty: Integer)
    // codeunit 50101 Sub: subscriber 'OnPair' HandlePairQty(var Rec: Record Cust; Qty: Integer)
    //   begin Rec.Validate(Name, 'x'); end
    // Precondition: each overload (told apart by parameters.len()) has exactly one
    // event-dispatch edge to HandlePairQty, resolution "resolved".
    // ctx_roles: OnPair/1 (1 parameter): expect_all(PRESENCE, "Unknown")
    // OnPair/2: expect(&[("validatesParam", "Yes"), ("mutatesParam", "Yes")])
}

/// Review Focus 1: `Sender` binds no publisher parameter and is not counted.
#[test]
fn review_sender_parameter_keeps_the_binding_exact() {
    // Pub: [IntegrationEvent(true, false)] OnSend(var Rec: Record Cust)
    // Sub: subscriber 'OnSend' HandleSend(Sender: Codeunit Pub; var Rec: Record Cust)
    //   begin Rec.Modify(); end
    // Precondition: one resolved dispatch edge OnSend -> HandleSend.
    // OnSend: expect(&[("persistsCurrentRecord", "Yes"), ("mutatesParam", "Yes")])
}

/// Review Focus 2: only the parameter the subscriber declares composes.
#[test]
fn review_partial_binding_leaves_the_other_parameter_alone() {
    // Pub: OnTwoRecs(var A: Record Cust; var B: Record Cust)
    // Sub: HandleB(var B: Record Cust) begin B.Modify(); end
    // role(OnTwoRecs, 1): expect(&[("persistsCurrentRecord", "Yes")])
    // role(OnTwoRecs, 0): expect_all(PRESENCE, "No")
}
```

  Run `cargo test --test gap gap_issue107` and `cargo test -p al-sem --lib issue107` (lib compiles: every placed lib test names only symbols from Tasks 2-4) → the listed tests fail for the parked reasons (`want Yes, got No`).
- [ ] **Step 2: Implement** `event_dispatch.rs`, the prologue map (build once per solve, next to `routines_by_id`; fill every `SccComputeCtx { … }` literal), `fold_callee_presence`, and the two fold call sites.
- [ ] **Step 3: Green.** All tests of Step 1 pass. The other gap rows stay red (they need Task 5's path facts). `cargo test -p al-sem --lib` green.
- [ ] **Step 4: Goldens.** Per Global Constraints (check without regen, `golden-diff-triager`, receipt, regen, check green). Expected movers: presence facts of bodied publishers and their callers only.
- [ ] **Step 5: Discrimination.** (i) skip the dispatch fold in `compose_roles_only` → every gap row of Step 1 fails; (ii) `optional_load` returns its input → `row5_manual_loader_is_masked` fails (`OnOptLoad.loadsFromDbParam` `Yes`); (iii) drop the overload-collision check → `row7d` fails on `OnPair/1`; (iv) drop the `sender` exclusion → `review_sender_…` fails; (v) revert Task 1's accumulation to the overwrite → `row5d_prime_…` fails on `FwdThenNoOp`, passes on `NoOpThenFwd` (record the split); (vi) drop the `!routine.body_available` skip in `dispatch_bindings` → `issue107_symbol_only_dependency_publisher_roles_unchanged` fails; (vii) #70's mutations, re-run with their recorded outcomes: (a) the solver writes back walked roles for `fixed_roles` members → `dependency_publisher_roles_stay_unknown` fails; (b) an empty `fixed_roles` only at the detector-context call → (ii) of that test fails, (iii) passes; (c, new) skip the dispatch fold for `fixed_roles` members → `issue107_dependency_publisher_presence_composes_subscribers` fails. Revert each.
- [ ] **Step 6: Commit.** rustfmt touched files; `scripts/ci-steps task` green; `git add` the code, the two test files, `tests/gap/main.rs`, moved goldens; `git commit -m "#107: event publishers fold their subscribers' presence facts"`.

### Task 5: The walker step

**Files:**
- Modify: `src/engine/l4/cfg_walker.rs` (`walk_param` `:231`, `apply_call` `:1193`, the caller at `:1872`)
- Modify: `src/engine/l4/event_dispatch.rs`
- Modify: `src/engine/l4/summary_runner.rs` (the `walk_param` call `:757`)
- Goldens: per Global Constraints
- Test: `tests/gap/gap_issue107_publisher_roles.rs` (place the remaining parked rows and fixtures); `event_dispatch.rs` unit test

**Interfaces:**
- Consumes: `ParamDispatch`, `DispatchKind`, `ExactBinding` (Task 4).
- Produces: in `event_dispatch.rs`, `pub(crate) fn dispatch_sequences(bindings: &[ExactBinding]) -> Vec<Vec<usize>>`: for every subset `S` of the optional bindings and every order of (unconditional bindings ∪ `S`), the list of binding indices in run order; the empty sequence appears only when there is no unconditional binding. In `cfg_walker.rs`: `pub(crate) struct CalleeLookup<'a> { pub snapshot: &'a HashMap<String, RoutineSummary>, pub final_map: &'a HashMap<String, RoutineSummary>, pub body_avail_by_id: &'a HashMap<String, bool> }`; `fn opaque_transfer(state: PerParamState, caller_is_var: bool, callee_is_var: bool) -> PerParamState` (the two duplicated opaque blocks of `apply_call`); `fn apply_callee_role(state: PerParamState, callee_id: Option<&str>, callee_param_index: u32, caller_is_var: bool, callee_is_var: bool, look: &CalleeLookup) -> PerParamState` (everything in `apply_call` from the opaque guard down; `apply_call` keeps the binding search and calls it); `fn apply_dispatch(state: PerParamState, pd: &ParamDispatch, look: &CalleeLookup) -> PerParamState`; `walk_param` gains `dispatch: Option<&ParamDispatch>` (the routine's entry for this parameter; `None` at `:1872`).

`apply_dispatch` (Design 4): `Unseen`, or `Exact` with any `path_opaque` binding → `opaque_transfer(state, pd.caller_is_var, callee_is_var)` once, where `callee_is_var` is `true` for `Unseen` and "any binding `callee_is_var`" otherwise. Else `join_states` over `dispatch_sequences`, each sequence folding `apply_callee_role(…, Some(&b.subscriber), b.q, pd.caller_is_var, b.callee_is_var, look)` left to right from `state` (no join inside a sequence; the empty sequence is `state`). `walk_param` maps every exit and fallthrough state through `apply_dispatch` before aggregating `all_states`. No closure, `LOOP_BOUND` or `saturate_unknown`.

- [ ] **Step 1: Place the tests; confirm red.** Gap rows (they turn green here): `row1_modify_subscriber_equals_a_direct_call`, `row2_validate_subscriber_dirties_the_publisher`, `row4_two_unconditional_loaders`, `row4b_unconditional_loader_plus_manual_noop`, `row5d_manual_copy_mutates_without_loading`, `row6_isolated_validate_modify_error`, `row7_event_id_collision_is_unseen`, `row7b_opaque_copy_absorbs_exact_edge`, `row7c_duplicate_resolved_edge_counts_once`, `row9_trigger_raiser_gains_no_role`, `row11_omitted_and_by_value_subscribers`, `row12_five_subscribers_are_unseen`, `row12b_unseen_dispatch_drops_the_callers_narrowing`, `plan_subscriber_forwarding_another_event`. Extend Task 4's `row7d`: `OnPair/1` also `expect_all(PATH, "Unknown")`. New tests:

```rust
// event_dispatch.rs #[cfg(test)]
#[test]
fn dispatch_sequences_cover_every_subset_and_order() {
    // b(optional) builds an ExactBinding with that flag.
    // 4 optional -> 65 sequences (1 + 4 + 12 + 24 + 24), all distinct
    // 4 unconditional -> 24, each of length 4
    // [b(false)] -> vec![vec![0]]
    // [b(false), b(true)] -> exactly {[0], [0, 1], [1, 0]} (no empty sequence: row 4b)
    // [b(true)] -> exactly {[], [0]}
}
```

```rust
// gap file
/// Review Focus 3: a resolved subscriber without a body is visible but opaque.
#[test]
fn review_bodyless_subscriber_is_opaque() {
    // model(&[SIMPLE, SIMPLE_SUB]); routines clone with HandleSimple.body_available = false;
    // solve(&m, &routines, &graph_of(&m))
    // OnSimple: expect(&[("persistsCurrentRecord", "Unknown"), ("requiresLoadedAtEntry", "Unknown")])
}

/// Review Focus 5: a by-value publisher parameter composes the entry
/// requirement only (c1a), no presence fact.
#[test]
fn review_by_value_publisher_param_composes_entry_only() {
    // Pub: OnVal(Rec: Record Cust); Sub: HandleVal(var Rec: Record Cust) begin Rec.Modify(); end
    // OnVal: expect(&[("persistsCurrentRecord", "No"), ("mutatesParam", "No"), ("requiresLoadedAtEntry", "Yes")])
}
```

  Run → the listed rows fail with the parked excerpts; the unit test does not compile (`dispatch_sequences` missing).
- [ ] **Step 2: Implement** `dispatch_sequences`, `CalleeLookup`, `opaque_transfer`, `apply_callee_role`, `apply_dispatch`, and the `walk_param` change; `compose_roles_only` passes each parameter's `ParamDispatch`.
- [ ] **Step 3: Green.** Every gap test passes (`cargo test --test gap gap_issue107`); `cargo test -p al-sem --lib` green.
- [ ] **Step 4: Goldens.** Per Global Constraints; expected movers now include path facts (`dirtyAtExit`, entry requirements, loaded fields) and d37/d39/d41/d42 findings at raising call sites.
- [ ] **Step 5: Discrimination.** (i) `walk_param` ignores `dispatch` → `row1` fails; (ii) `dispatch_sequences` always adds the empty sequence → `row4b` and the unit test fail; (iii) treat `path_opaque` as exact → `row6` fails; (iv) drop the `too_many` rule → `row12` fails; (v) do not let an Opaque edge absorb an Exact one (Task 4 rule) → `row7b` fails; (vi) drop the `(S, q)` dedupe → expect `row7c` to fail; if it stays green (a second `Modify` on `Persisted` may change nothing), record it as a stated limit of that row; (vii) fold the bindings in one fixed order with a join after each (the round-1 closure) → `row3` fails (`dirtyAtExit` `Yes`). Revert each.
- [ ] **Step 6: Commit.** rustfmt touched files; `scripts/ci-steps task` green; `git commit -m "#107: the walker applies an event dispatch by exact enumeration"`.

### Task 6: Capped roles SCC publishes the opaque role; dispatch census

**Files:**
- Modify: `src/engine/l4/summary_runner.rs` (`run_one_scc_roles` cap branch `:1790-1830`, `summaries_census` statics and `report()`)
- Modify: `src/engine/l4/event_dispatch.rs`, `src/engine/l4/cfg_walker.rs` (counters), `src/program/model/events.rs` (mixed-readings counter)
- Test: `issue107_pending_tests` (place `forced_roles_cap_publishes_the_opaque_role` and its `use` lines)

**Interfaces:**
- Produces: `fn roles_iteration_cap() -> usize` (`MAX_FIXED_POINT_ITERATIONS`, or the test override); `#[cfg(test)] pub(crate) fn with_roles_iteration_cap<R>(cap: usize, f: impl FnOnce() -> R) -> R` (a `thread_local!` `Cell<Option<usize>>`, restored on exit by a drop guard; the solver is single-threaded); `fn opaque_role(r: &RecordRoleSummary) -> RecordRoleSummary` (a clone with the 11 `EffectPresence` fields `Unknown` and `required_loaded_fields_at_entry`, `current_loaded_fields_at_exit` `FieldList::Unknown`; identity and syntactic fields kept).

- [ ] **Step 1: Place the test; confirm red** (compile error on `with_roles_iteration_cap`).
- [ ] **Step 2: Implement.** The cap check uses `roles_iteration_cap()`; on a cap every member's roles in `out` go through `opaque_role`. The `fixpoint-capped` uncertainty and the `SummarizeDiagnostic` stay. Census counters (only counted when the census is on; `fetch_max` for maxima): dispatching publishers; bindings per (publisher, parameter) as buckets 1/2/3/4/>4 and max; `ParamDispatch` taking exact / unseen-opaque / unseen-too-many / isolated-path-opaque; sequences enumerated (total, max per `apply_dispatch`); roles Jacobi rounds in SCCs with a member in `dispatch_by_id`; dispatching publishers with a non-empty body (`!record_operations.is_empty() || !call_sites.is_empty()`, expected 0); subscriptions whose `conditional_any != !conditions.is_empty()` (counted at the `events.rs` push). One `report()` line each.
- [ ] **Step 3: Green.** The test passes; `cargo test -p al-sem --lib` green; `bash scripts/check-goldens > <log> 2>&1` green with zero moves (no corpus SCC caps).
- [ ] **Step 4: Discrimination.** Drop the `opaque_role` replacement → the test fails (`OnCyc.persistsCurrentRecord: want Unknown`); make `with_roles_iteration_cap` a no-op → it fails on its `!diags.is_empty()` precondition. Revert each. The counters change no output: no test.
- [ ] **Step 5: Commit.** rustfmt; `scripts/ci-steps task` green; `git commit -m "#107: a capped roles SCC publishes the opaque role; dispatch census"`.

### Task 7: CHANGELOG, branch gates, CDO measurement

**Files:**
- Modify: `CHANGELOG.md` (`## [Unreleased]`: `### Fixed` the publisher roles; `### Changed` one parameter mapping (quotes ignored), the c1b accumulation, the capped-SCC opaque role; `### Added` the dispatch census counters)

- [ ] **Step 1: Every parked test is placed.** List the `fn` names under `#[test]` in `.agent/issue-107/pending-tests/` and in the committed files; the first list is a subset of the second. Leave the stash in place and name it in the report.
- [ ] **Step 2: CHANGELOG** entry (plain English; name the stated leniencies and their owners: the d41 / `record_load_points` bodyless guard and the order-blind `validates && Unknown` rule, both discoveries to file).
- [ ] **Step 3: Gates.** `bash scripts/check-goldens --verify-coverage`, `bash scripts/check-goldens`, `bash scripts/ci-steps all`, `bash scripts/cdo-gate U:/Git/DO-cdo-baseline/Cloud`, each to a log, all green.
- [ ] **Step 4: CDO measurement** (`cdo-measure` skill, release-fast `alsem` from base `8405f96d` and from the branch head): the spec's Measurement plan steps 1-6. Must hold: every non-role detector and `d3` byte-identical; roles cap hits 0 and 0; histogram, db-effect and cone projections byte-identical; non-empty publisher bodies 0. Triage every new d39/d40/d41/d42 finding (`triage-findings`); a detector over 30% false positives on its new findings: fix the cause. Report the numbers in the task report.
- [ ] **Step 5: Commit.** `git add CHANGELOG.md`; `git commit -m "#107: CHANGELOG"`.
