# Multi-Source Reverse Walk for impact's up Pass Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace `impact`'s 699 per-root reverse walks with one bit-parallel multi-source walk.
Every root's `up` row must stay byte-identical. The target is 8-12 % of `impact --diff`'s CPU on
CDO, and the bar is at least 5 %.

**Architecture:**
- **The walk.** `reverse::multi_walk` traverses the union state graph once per BFS level, with
  each state carrying the roots that reach it as a bit set. It processes each level's states in
  sorted state order and each state's sites in `sites_into` order, exactly as every per-root walk
  does. So each root receives the same sequence of reach, step and rejoin events as its own walk.
- **The visitor.** A bit-set-aware visitor, `UpMulti`, consumes the events. Reachability-only
  fields come from per-node and per-entry root masks (pass 1). Fields that need a node's *final*
  class, or a bounded list of first arrivals, come from a second traversal with pass 1's final
  masks known (pass 2).
- **The oracle.** The per-root walk stays as the test oracle, never as a production path.

**Tech Stack:** Rust, the al-sem crate (`src/program/impact_graph/reverse.rs`,
`src/engine/impact/up.rs`), `crate::fxhash`, and the existing impact goldens and CDO ratchets.

**Spec:** `docs/superpowers/notes/2026-11-17-eng125-shared-walk.md` (revision 2). Prototype and
measurements: branch `tools/eng-125-proto` (db28c331, never lands) and
`/work/.al-sem/profiles/2026-11-17-eng125/`.

## Global Constraints

- Code starts only after ENG-128 lands: it adds the every-arrival `ConeVisitor` hook this plan
  builds on (orchestrator, condition 1).
- Every root's `up` JSON is byte-identical to today's, per field, with a test per field
  (condition 2).
- At least 3 more clean timing rounds of the ceilings before code (condition 3; Task 0).
- Two-model review of this plan before code (condition 4).
- No dual production path (owner principle 16): the per-root walk survives only as a
  `#[cfg(test)]` oracle and in the CDO comparison test.
- Bar for landing: impact CPU down at least 5 % on CDO (3+ alternating clean rounds), outputs
  byte-identical, `cdo-before-after` unmoved, `cdo-speed` twice PASS, Fable and sol review.

## Review Focus

1. **A root with several nodes** (`#if` arms; none on CDO, 0 of 699). `walk_multi` labels each
   state with its nearest arm, and ties go by arm order. The multi walk must reproduce the
   per-arm order (lanes, Task 1). A fixture test pins it.
2. **The universal hub** (none on CDO). It is injected once per root after level 0, with tier
   `OnlyOpen`. A fixture test pins it.
3. **A root that hits the state budget** (none on CDO). Its bit must stop at the same processed
   state, mid-level, in sorted order, and report `limited`. A fixture test with a small budget
   pins it.
4. **The table-event guard.** A guarded state is a different state, and a raising site that
   proves the guard is not crossed. The multi walk must use `walk_multi`'s exact guard rule. A
   fixture with a temporary-record raise pins it.
5. **Ties at equal key.** Two entries or nodes with the same (class, tier, hops) are decided by
   per-root arrival order `at`. The pass-2 slot fill must take the first one in processing order.
   A fixture with two equal-key entries pins it.

---

## Per-field plan (condition 2)

Notation:
- **P1:** pass 1, root masks.
- **P2:** pass 2, the same traversal with P1's final masks known.
- **`at`:** the per-root processing index. Levels go in order, states in sorted order within a
  level, and the root's own states are counted.

| `up` field | how it is computed | why it is exact | test (Task 4/5) |
|---|---|---|---|
| `states`, `maxHops` | P1: count of a root's processed states; last level with its bit | the same level sets (Task 1) | `multi_states_and_max_hops_equal_oracle` |
| `ancestors.{local,throughDependency,onlyOpen,total}` | P1: per node, `tier_mask[t] \|= bits`; a root's best tier = the lowest t with its bit | `NodeBest.tier` is the minimum tier over reached states | `multi_ancestor_tiers_equal_oracle` |
| `hidden`, `rootEntryKinds/Word/UserOrJob`, `hubs[].at` | P1: per node "reached" mask; hub `at` = min anchor over sites from reached nodes | reachability only | `multi_hidden_root_entry_hubs_equal_oracle` |
| `byRelation`, `entriesByKind`, `entriesByContext`, `programShare`, `entryNotes`, `externalCode` | P1: per node and per canonical entry, `pop_mask[p] \|= bits` (pop from tier and ext); best pop = lowest p | `Witness::key` puts population first; the counts use only the final class | `multi_classified_counts_equal_oracle` |
| `entries[].count/onlyOpen`, `external[].count/onlyOpen`, `userOrJobEntries`, `entryBuckets` | P1: per (node, kind) and (node, why) masks `any` and `non_open` | `Firsts` counts distinct nodes; only-open = `any & !non_open` | `multi_firsts_counts_equal_oracle` |
| `eventPublishers.*` | ENG-128's `count_publisher(site, hops, tier)`, fed by first arrivals (`reach`'s `Via::Site`) and by `rejoin` (dropped arrivals): P1 masks for counts, P2 slots for `first` | after ENG-128 a publisher counts on any event-dispatch arrival, first or dropped | `multi_event_publishers_equal_oracle` |
| `entries[].first`, `external[].first`, `eventPublishers.first`, `.direct.first` | P2: per (root, list) bounded slots: the first `keep.per_list` distinct nodes, non-open ones first, in processing order. A root's bit leaves a list's active mask once the list is full of non-open nodes | `Firsts::first` sorts by (open, hops, at), and processing order is (hops, at) | `multi_first_lists_equal_oracle` |
| `entries[].first[].action/caption` | P2: `action_site` for the (root, node) pairs in the lists: the first action-run step with `st.from == n` in processing order | `Up::step` keeps the first `or_insert` | `multi_page_action_captions_equal_oracle` |
| `firstByKind`, `nearest` (entry case) | P2: per (root, word, pop p, tier rank t) slot: the first arrival of an entry whose P1 final pop is p, arriving with pop p and rank t; then min over t | `(key, at)` minimum with key = (p, t, hops), and processing order is (hops, at) | `multi_first_by_kind_and_nearest_equal_oracle` |
| `nearest` (ancestor fallback) | P2: the same per (root, p, t) over non-root, non-hub nodes whose final pop is p | the same | in the test above |
| `nearestAncestors` (+ hops) | P1: the first `keep.ancestors` distinct non-root nodes with a stable id, in processing order. P2: each one's hops at its best tier (the first arrival at the P1 best tier) | `Up::reach` pushes in arrival order; `NodeBest.hops` = hops at the minimum (tier, hops) | `multi_nearest_ancestors_equal_oracle` |
| `limited` | budget per root, counted in processing order | Task 1 | `multi_state_budget_equal_oracle` |
| everything else (`callers`, `display`, `nodes`, `programEntries`, `programEntriesByKind`) | unchanged, not from the walk | — | the whole-row test |

Each per-field test runs the oracle (today's per-root `walk_root` row) and the multi row on the
same fixtures and compares that field's JSON. The whole-row test compares every row, byte for
byte.

## File structure

- `src/program/impact_graph/reverse.rs`: `multi_walk`, `RootBits`, `MultiSink` (the event
  interface); the per-root `walk_multi` is unchanged.
- `src/program/impact_graph/multi_walk_tests.rs`: Task 1-2 tests (oracle event-sequence equality).
- `src/engine/impact/up_multi.rs`: `UpMulti`, the P1 masks, the P2 slots, `rows()`, which
  builds the same JSON as `summary` and `walk_root`. It reuses `summary`'s helpers (`node_json`,
  `witness_json`, `by_relation`, `share_json`), made `pub(super)`.
- `src/engine/impact/up_multi_tests.rs`: the per-field tests in the table above.
- `src/engine/impact/up.rs`: `up()` calls `UpMulti` instead of the per-root pass. `walk_root`
  moves under `#[cfg(test)]` as the oracle, used by the tests and by the CDO comparison.

---

### Task 0: Three more clean ceiling rounds (condition 3)

**Files:** none (prototype binary from `tools/eng-125-proto`).

- [ ] **Step 1:** Run `scratchpad/e125-proto3.sh` (default / noop / skip, rounds 5-7, rotated
  order, host CPU pressure logged) in the CDO slot.
- [ ] **Step 2:** Drop any round with sys > 30 s or host `some avg10` > 15 %.
- [ ] **Step 3:** With at least 3 clean rounds in all (rounds 1, 4 and the new ones), report the
  means and ranges of default − skip (the per-root pass) and noop − skip (its traversal).
  **Stop and report** if the per-root pass is under 8 %: the target then cannot clear the bar.

### Task 1: `multi_walk`, the traversal core

**Files:**
- Modify: `src/program/impact_graph/reverse.rs`
- Create: `src/program/impact_graph/multi_walk_tests.rs`

**Interfaces:**
- Produces:
  ```rust
  pub struct RootBits(pub Box<[u64]>); // one bit per logical root
  pub trait MultiSink {
      fn reach(&mut self, s: WalkState, hops: u32, via: Via, from: Option<WalkState>, roots: &RootBits);
      fn step(&mut self, from: WalkState, site: SiteIx, roots: &RootBits) -> u8; // ext bits; pure in (from, site)
      // ENG-128's `ConeVisitor::rejoin`: an arrival the walk drops (the state is already
      // seen for that root), after the follow_open, guard and step filters; `hops` = parent + 1.
      fn rejoin(&mut self, n: WalkState, hops: u32, site: SiteIx, roots: &RootBits);
      fn limit(&mut self, root: usize, l: Limit);
  }
  pub fn multi_walk(g: &SitedGraph, roots: &[Vec<NodeIx>], opts: &WalkOptions<'_>, sink: &mut dyn MultiSink) -> Vec<WalkStats>;
  ```
  The `report` side effects (hidden, entry kinds, external, source) are emitted inside `reach` as
  `walk_multi`'s `report` does, with the same arguments.
- **Lanes for multi-node roots:** each node of a root has its own lane, ordered by node position
  as `walk_multi`'s `roots` slice is. Within a level, lanes go in order. A state first reached by
  lane i blocks it for lanes > i of the same root. That gives `walk_multi`'s "nearest root, ties
  by root order".

- [ ] **Step 1: Write the oracle tests**
  - `multi_walk_event_sequence_equals_per_root_walks`: on the `World`-built graphs of
    `src/engine/impact/up_tests.rs` and `src/program/impact_graph/reverse_tests.rs`, plus three new
    `World` fixtures (a table-event guard, a universal hub, a two-arm root), and on the sited graph
    of `tests/fixtures/test-universe` (`cli_impact`'s fixture). For each root, demultiplex the events whose bit is set. Assert the sequence of
    `(reach state, hops, via, from)`, `step (from, site)` and `rejoin (n, hops, site)` equals the
    sequence a recording `ConeVisitor` sees from that root's `walk_one` or `walk_multi`. Lengths
    must be equal too.
  - `multi_walk_budget_stops_each_root_where_its_walk_does`: budget 7 on a fixture where two
    roots exceed it. Equal event prefixes, and `limit` for those roots only.
- [ ] **Step 2:** Run `cargo test -p al-sem --lib multi_walk`. Expected: FAIL (no `multi_walk`).
- [ ] **Step 3: Implement `multi_walk`**
  - Intern states in an FxHashMap, with per-state `seen` bits.
  - Each level is a `Vec<(state_ix, RootBits)>` sorted by `WalkState`'s `Ord`.
  - Per site: apply `walk_multi`'s open, guard and `step` rule. Call `rejoin` with
    `parent_bits & seen[child]` (the roots that drop this arrival), taken BEFORE the new bits are
    set. Then `new = parent_bits & !seen[child]`, set them, and add the child to the next level.
  - The universal hub goes after level 0 for every non-empty root.
  - Budget: a per-root processed counter; a root's bit is cleared from the rest of the run when
    it reaches the budget.
  - Cancellation: checked every `CANCEL_EVERY` processed (state, word) units.
- [ ] **Step 4:** Run the tests. Expected: PASS.
- [ ] **Step 5: Discrimination proofs, each red then green**
  - The level sort by `state_ix` instead of `WalkState` → the sequences differ.
  - Lanes collapsed into one bit → the two-arm fixture differs.
  - The universal injection dropped → the hub fixture differs.
- [ ] **Step 6:** Commit.

### Task 2: `UpMulti` pass 1, the reachability fields

**Files:**
- Create: `src/engine/impact/up_multi.rs`, `src/engine/impact/up_multi_tests.rs`
- Modify: `src/engine/impact/up.rs` (helpers `pub(super)`)

**Interfaces:**
- Consumes: Task 1's `MultiSink`, `multi_walk`.
- Produces: `pub(super) struct UpMulti<'g>` with
  `fn new(g, keep, roots: &[Vec<NodeIx>], shared) -> Self`, implementing `MultiSink`.
  - Per node: `reached`, `tier[3]`, `pop[6]`, `kind_any[13]`, `kind_nonopen[13]`,
    `why_any[3]`, `why_nonopen[3]`, `hidden` (`RootBits` each).
  - Per canonical entry: `pop[6]`.
  - Per root: `states`, `max_hops`, `root_entry`, and the first `keep.ancestors` list.

- [ ] **Step 1:** Write the P1 tests from the table (`multi_states_and_max_hops_equal_oracle`,
  `multi_ancestor_tiers_equal_oracle`, `multi_hidden_root_entry_hubs_equal_oracle`,
  `multi_classified_counts_equal_oracle`, `multi_firsts_counts_equal_oracle`). Each compares that
  field's JSON from the oracle row and from `UpMulti::rows()`, on Task 1's fixtures.
- [ ] **Step 2:** Run them. Expected: FAIL.
- [ ] **Step 3:** Implement the P1 sink and the count folds in `rows()`. A fold iterates nodes
  (or entries) by words, and adds 1 per set bit to that root's counter. A root's own nodes are
  removed per root (`root_nodes`).
- [ ] **Step 4:** Run the tests. Expected: PASS.
- [ ] **Step 5:** Discrimination proofs, each red then green:
  - the best tier taken as the highest set tier;
  - a root's own node not removed;
  - `only_open` computed as `non_open & !any`.
- [ ] **Step 6:** Commit.

### Task 3: `UpMulti` pass 2, the first-arrival and witness fields

**Files:** Modify `src/engine/impact/up_multi.rs` and `src/engine/impact/up_multi_tests.rs`.

**Interfaces:**
- Consumes: P1's final masks (best tier, best pop per (root, node) and (root, entry)).
- Produces, in `UpMulti`:
  - `fn pass2(&mut self) -> impl MultiSink + '_`;
  - per root: the slots `first_lists[list] -> Vec<(NodeIx, open, hops, tier)>` (at most
    `keep.per_list`);
  - `class_slots[(word|ancestor, pop, tier rank)] -> Option<(id, Witness)>`;
  - `action_site[(node)]` for listed nodes only;
  - each list's `active: RootBits` (the roots whose slots are not yet filled).

- [ ] **Step 1:** Write the P2 tests from the table (`multi_first_lists_equal_oracle`,
  `multi_page_action_captions_equal_oracle`, `multi_first_by_kind_and_nearest_equal_oracle`,
  `multi_nearest_ancestors_equal_oracle`, `multi_event_publishers_equal_oracle`, plus Review
  Focus 5's `multi_equal_key_ties_take_the_first_arrival`).
- [ ] **Step 2:** Run them. Expected: FAIL.
- [ ] **Step 3: Implement pass 2**
  - Run `multi_walk` again (deterministic). For each event, only the bits in `roots & active` are
    visited one by one; full slots clear their bit from `active`, so the per-bit work is bounded
    by the slots.
  - Slots fill strictly in event order, so the first arrival wins ties, as `<` does in `Up`.
- [ ] **Step 4:** Run the tests. Expected: PASS.
- [ ] **Step 5:** Discrimination proofs, each red then green:
  - a slot filled by the last arrival;
  - the pass-1 final pop ignored (the arrival pop used instead);
  - `action_site` taken from the target node.
- [ ] **Step 6:** Commit.

### Task 4: Wire into `up()`; the whole-row oracle; the CDO comparison

**Files:**
- Modify: `src/engine/impact/up.rs`
- Test: `src/engine/impact/up_multi_tests.rs`, `tests/cli/output_cdo.rs` (or the impact ratchet
  module)

- [ ] **Step 1: Tests**
  - `multi_rows_equal_oracle_rows`: every row's JSON byte-equal to `walk_root`'s, on all
    fixtures.
  - A CDO test (`cdo` in its name, through `cdo_ws_or_enforce`): `impact --diff` on the pinned
    baseline; all 699 rows byte-equal between the oracle and production. It runs on `cdo-gate`
    only.
- [ ] **Step 2:** Replace the per-root pass in `up()` with
  `UpMulti::new(..)` → `multi_walk` (P1) → `pass2` → `multi_walk` (P2) → `rows()`.
  `walk_root` moves to `#[cfg(test)]`. Keep the `impact.up.*` counters and trace spans; add
  `impact.up.multi_ms`.
- [ ] **Step 3:** Run `scripts/check-goldens` and `cargo test -p al-sem --lib impact`. Expected:
  every impact golden unchanged.
- [ ] **Step 4:** Commit.

### Task 5: Measure and gate

- [ ] **Step 1:** `cdo-before-after <master> <head>`: nothing moved. Impact outputs byte-identical
  over 6+ timed runs (sha256 per run).
- [ ] **Step 2:** 3+ alternating clean rounds of the 699-root `impact --diff`. Claim a CPU ratio
  only if the pairs agree (memory rule: two runs that agree). Peak RSS within cdo-speed's 1.10.
- [ ] **Step 3:** `cdo-speed` twice; the CDO subset (`cdo-gate --only harness,l4,output`).
- [ ] **Step 4:** CHANGELOG; Fable, then sol; the task gate; submit.
