# Multi-Source Reverse Walk for impact's up Pass Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

Revision 3, after two rounds of two-model review (Fable and gpt-6.1-sol, 2026-11-17):
- Revision 1: both reviewers rejected the event model and the slot rule.
- Revision 2: both rejected the hops-based slot close for keys that do not start with hops.
  Fable also rejected chunks per worker, since the traversal is paid once per chunk.
- Revision 3 partitions every slot by its leading key, measures the chunk count, and adds a
  memory go/no-go.

**Goal:** Replace `impact`'s per-root reverse walks AND its union walk with one bit-parallel
multi-source walk. Every root's `up` row and the union summary must stay byte-identical (cost and
timing fields excepted, §Cost fields). The target is 8-12 % of `impact --diff`'s CPU on CDO, and
the bar is at least 5 % with wall time not worse.

**Architecture:**
- **The walk.** `reverse::multi_walk` runs one traversal of the union state graph per BFS level
  for a chunk of logical roots. Each next-level arrival is a record `(lane, child, from, site,
  new_bits)`. A level is processed in `(lane, child)` order, records of one child in arrival
  order. So every root sees exactly the event sequence of its own `walk_multi` (Task 1 proves it
  by trace equality).
- **The visitor.** `UpMulti` consumes the events. It folds root bit masks per reached node and
  per canonical entry (pass 1), and fills witness slots with the real keys (pass 2).
- **Union and workers.** The union walk becomes one more logical root, whose lanes are the sorted,
  deduplicated `all_nodes`, so `Up`, `summary` and `classified` leave production.
  - The roots are split into chunks, each walked by one worker.
  - Every chunk crosses nearly the whole giant component, so the traversal CPU grows with the chunk
    count.
  - The chunk count is therefore a MEASURED decision (Task 2): the fewest chunks that keep wall
    time within cdo-speed's limit.

**Tech Stack:** Rust, the al-sem crate (`src/program/impact_graph/reverse.rs`,
`src/engine/impact/up.rs`), `crate::fxhash`, the impact goldens, and the lib CDO phase of
`cdo-gate`.

**Spec:** `docs/superpowers/notes/2026-11-17-eng125-shared-walk.md` (revision 2). Prototype:
`tools/eng-125-proto` db28c331 (never lands). Measurements: `/work/.al-sem/profiles/2026-11-17-eng125/`.

## Global Constraints

- **Code starts after ENG-128 lands** (condition 1). This plan builds on its hook:
  `ConeVisitor::rejoin(root, s, hops, site)`, called for an arrival the walk drops (the `else` of
  `scratch.insert(n)`), after the follow_open, guard and step filters, with hops = parent + 1. It
  also builds on `Up::count_publisher(site, hops, tier)`, fed by `reach`'s `Via::Site` and by
  `rejoin`.
- **Byte-identical per field, a test per field** (condition 2; the table below).
- **3+ more clean ceiling rounds before code** (condition 3; Task 0).
- **Two-model review of this plan before code** (condition 4).
- **No dual production path** (principle 16). The per-root walk with `Up` survives only as a
  `#[cfg(test)]` oracle, which the lib tests and a lib CDO test (`cdo` in its name, the `--lib cdo`
  phase of `cdo-gate`) both reach.
- **Test selection's own walk** (`test_select::select`) is out of scope and unchanged.
- **The bar for landing:** impact CPU down at least 5 % on CDO over 3+ agreeing clean rounds,
  wall time not worse (cdo-speed's 1.30 limit), peak RSS within 1.10, outputs byte-identical,
  `cdo-before-after` unmoved, `cdo-speed` twice PASS, then Fable and sol.

## Review Focus

1. **Mixed-hop events within a level.** Action-run entries, `canon.add` from `step` and `rejoin`
   carry hops + 1 while level h is being processed. A slot must not keep an h+1 item over a later
   h one. Task 3 has a fixture with an early h+1 action-run arrival and a later h entry.
2. **Two roots reaching one state from different parents** at the same level: each root's `via`
   and `from` must be its own. Task 1 has a fixture.
3. **Lanes:**
   - **Order:** a level is sorted by (lane, state).
   - **One visited set per logical root:** a state seen by any lane is blocked for every lane.
   - **A state can sit in a level twice,** for different roots on different lanes.
   - Task 1 has a two-arm fixture with unequal distances.
4. **The state budget's exact boundary.**
   - `walk_multi` fires `limit` only when a further state would be processed.
   - A root with exactly `budget` states is not limited.
   - The last permitted state still runs all its steps and entries.
   - Task 1 tests budgets 0, exact and exact + 1.
5. **Crossed best tier and best class.** A node's legacy best tier and its class witness can come
   from different states (local navigation against dependency code). `byRelation` needs the joint
   (population, tier rank) masks. Task 2 has a fixture.

---

## Event model (Task 1)

`multi_walk` emits, per logical root bit set, exactly what a `ConeVisitor` gets from that root's
`walk_multi`, in the same order:

- **`reach(s, hops, via, from)`, then `report`'s calls**, when a level record is processed:
  `hidden` once per (root, node), `entry` with the node's kinds plus the overlay and background
  kinds, `external`, and `source`.
- **`step(from, site)`, before the visited check.**
- **The action-run `entry(n, PageAction, hops + 1)`,** deduplicated per (root, child state) like
  `action_entries`, when the step's site is an action run.
- **`rejoin(n, hops + 1, site)`** for the roots whose bit is already in `seen[child]`.
- **The universal hub, after level 0:** one record per lane-0 root, when `follow_open`, the hub
  has callers, and the root is non-empty and not yet at the hub.
- **`limit`:** per root, when a further state would be processed past the budget.
- **Cancellation** is polled every `CANCEL_EVERY` processed records. A cancelled walk gives every
  unfinished root `limit(Cancelled)`, so its row keeps today's shape (`limited: "cancelled"`). The
  cut point may differ from today's per-root cut, which is fine: no caller compares a cancelled
  run's rows (the CLI never cancels, and the resident engine discards a stopped read).

## Per-field plan (condition 2)

Notation:
- **P1:** pass 1, the root-mask folds.
- **P2:** pass 2, the second traversal with P1's final masks known.
- **Slot** (revision 3; both reviews):
  - **One leading key per slot.** A slot is partitioned by the key's leading parts (open-ness for
    lists, (pop, tier rank) for class witnesses). It admits only arrivals whose leading parts equal
    the candidate's FINAL ones from P1, and keeps each arrival's own hops, tier and order.
  - **Inside a slot the key is (hops, event order).** A slot holds distinct ids (nodes or
    entries), keeping the smaller (hops, order) on a repeat.
  - **Close rule.** Hops now leads, so a full slot may close at the level boundary after its worst
    member's hops + 1 (events never carry more than hops + 1).
  - **The field's result** is the merge of its partitions in leading-key order.
  - **Per-bit work** is bounded: event bits are first ANDed with P1's final candidate mask for the
    slot. A per-(root, id) delivered mask is set only at the LEVEL BOUNDARY, from a pending mask
    kept during the level. Within a level, an arrival at h+1 can come before one at h for the same
    id (a `rejoin` before that publisher's `reach`; an action-run entry before the page's own
    entry; `canon.add` from `step` before `entry`). A later same-level arrival with smaller hops
    therefore still replaces it, under the "keep the smaller (hops, order)" rule. So the per-bit
    steps are about one P1 fold's size plus a level's repeats.
  - **Event order** is ONE per-root sequence shared by every slot of a field. `nearest` compares
    winners across words and across the two `overConfiguration` classes by it, so a per-slot
    counter would break the tie-break.

| `up` field | mechanism | exact because | test |
|---|---|---|---|
| `states`, `maxHops` | P1: count of delivered `reach` events per root; the last level with a reach | Task 1's trace equality | `multi_states_and_max_hops_equal_oracle` |
| `ancestors.{local,throughDependency,onlyOpen,total}` | P1: `tier[3]` per reached node; best = the lowest tier with the root's bit; roots AND hubs left out | `NodeBest.tier` is the minimum over reached states (up.rs summary) | `multi_ancestor_tiers_equal_oracle` |
| `hidden` | per node, static channels × `reached` | `report` calls it once per (root, node) | `multi_hidden_equal_oracle` |
| `rootEntryKinds/Word/UserOrJob` | per root: the kinds of the DELIVERED `entry` callbacks on its own nodes (none at budget 0; only the processed ones of a partly processed multi-node root) | `Up::entry` fills `root_entry` from the callback | `multi_root_entry_equal_oracle` (budget 0 and partial) |
| `hubs[].at` | per hub node reached: the minimum FORMATTED location string over sites from reached nodes | as summary computes it | `multi_hubs_equal_oracle` |
| `byRelation`, `entriesByKind`, `entriesByContext`, `programShare`, `entryNotes` | P1: joint `class[pop × tier rank]` masks per reached node and per canonical entry (both from `entry` → `inv.of_node` and `step` → `inv.of_site`); best = the lowest (pop, rank) | `Witness::key`'s order is (population, tier rank, hops) | `multi_classified_counts_equal_oracle` (with Review Focus 5's fixture) |
| `externalCode` | P1: per external why, nodes whose best class is Code; root nodes NOT left out | as up.rs computes it, from the unfiltered `best` | `multi_external_code_equal_oracle` |
| `entries[].count/onlyOpen`, `userOrJobEntries`, `entryBuckets` | per node static kinds × `reached`, `non_open = tier[Local] \| tier[Dep]`; PageAction from action-run arrivals adds two masks of its own; root nodes left out (`Up::entry`) | `Firsts` counts distinct nodes; only-open = `any & !non_open` | `multi_entry_counts_equal_oracle` |
| `external[].count/onlyOpen` | static whys × `reached`; root nodes kept (`Up::external`) | as above | `multi_external_counts_equal_oracle` |
| `eventPublishers.*` | P1 masks via `count_publisher` from reach and `rejoin`; `direct` per root (sites into its own nodes) | ENG-128's rule | `multi_event_publishers_equal_oracle` |
| `entries[].first`, `external[].first`, `eventPublishers.first`, `.direct.first` | P2, two partitions per (root, list). **Non-open:** non-open arrivals of nodes whose final category is non-open. **Open-only:** arrivals of nodes whose final category is `any & !non_open`, filled only while P1's non-open count is below `keep.per_list`. Each keyed (hops, order); the result is non-open then open-only, at most `keep.per_list` | `Firsts::add` keeps min (open, hops), first arrival on ties; `first` sorts by (open, hops, at). A node's witness is its first arrival in its final category (an open hop 2 then a non-open hop 10 gives non-open, hop 10) | `multi_first_lists_equal_oracle` (Review Focus 1's fixture, and the open-then-non-open upgrade) |
| `entries[].first[].action/caption` | P1: per page-action node, (site, bits) in event order from `step`; the first record with the root's bit gives `action_site` | `Up::step`'s first `or_insert`, independent of the lists | `multi_page_action_captions_equal_oracle` |
| `firstByKind`, `nearest` (entry) | P2, one slot per (root, word, pop p, tier rank t). Candidates are entries whose P1 final class is exactly (p, t), arriving with (p, t); keyed (hops, order). The result per (word, p) is the slot of the lowest t present | `classified`: min `(key, at)` with key (p, t, hops) among entries of final population p. A rank-2 member at hops 3 loses to a rank-0 arrival at hops 9 because the partitions are compared first | `multi_first_by_kind_and_nearest_equal_oracle` (with a rank-upgrade fixture: a full slot at low hops, then a better rank two levels later) |
| `nearest` (ancestor fallback) | P2, slots per (root, p, t) over non-root, non-hub nodes whose final class is (p, t), keeping the full witness with `ext` (`needsNavigation`) | the same | in the test above |
| `nearestAncestors` (+ hops) | P1: the first `keep.ancestors` distinct non-root nodes with a stable id, by reach order; P2: hops at the first reach at the node's final best tier | `Up::reach` pushes in arrival order; `NodeBest` keeps the min (tier, hops) | `multi_nearest_ancestors_equal_oracle` |
| `limited` | Task 1's budget rule | trace equality | `multi_state_budget_equal_oracle` |
| `callers`, `display`, `nodes`, `programEntries`, `programEntriesByKind`, not-in-graph rows | unchanged | not from the walk | `multi_rows_equal_oracle_rows` |
| **union summary** (`union`) | the same folds for the union logical root. Its lanes are `all_nodes` SORTED AND DEDUPLICATED, as `up()` builds them, not request order | the same | `multi_union_equals_oracle_union` (with reversed and duplicate root requests) |

### Cost fields

`cost.perRootStates`, `unionStates`, `walksUs`, `walkMs`, `visitorBytes` and `scratchBytes`:
- the state counts stay exact;
- the times and bytes now describe the multi walk;
- the comparisons blank them, as `up_par_tests.rs` already does.

### Memory bound

The masks are sparse by reached node and reached entry: 11 words (88 B) per family. Per node:
- `reached`;
- `tier[3]`;
- `class[18]`;
- page-action any/non-open (2);
- publishers any/non-open and direct (4).

That is 28 families × 23,066 reached nodes ≈ 57 MiB. The 18 class families × ~15,000 reached
entries ≈ 24 MiB.

Per chunk's traversal (bitset widths rounded to whole words):
- `seen`: ≤ 191,824 states × 88 B ≈ 16 MiB;
- `action_entries`: the same order;
- the state index;
- the pass 2 slots;
- the current and next level records. These scale with the new (root, state) pairs of a level,
  NOT with distinct states, so they are not yet bounded.

**This is an estimate, not a bound** (both reviews). Task 1 measures the peak records per level
on CDO. Task 2 has a memory go/no-go: the whole up pass's added peak against cdo-speed's 1.10 of
the one-shot's 3.5 GB, which leaves about 350 MiB.

---

### Task 0: Three more clean ceiling rounds (condition 3)

- [ ] **Step 1:** Run `scratchpad/e125-proto3.sh`: default / noop / skip, rounds 5-7, rotated
  order, host pressure logged.
- [ ] **Step 2:** Leave out a round with sys > 30 s or host pressure above 15 %.
- [ ] **Step 3:** From all clean rounds (1, 4 and the new ones), report the means and ranges of
  default − skip and noop − skip. **Stop and report** if default − skip is under 8 %.
  - **Done 2026-11-17.** Rounds 5-7 were all clean (sys ≤ 27 s, host pressure avg10 ≤ 2.3 %).
    default − skip was 17.8 %, 16.3 % and 16.4 %. Over the five clean rounds (1, 4, 5, 6, 7) the
    mean is 63.8 s, 15.6 % of impact's CPU; the per-root pass is above the 8 % threshold.
  - The visitor-only share (default − noop) stays noisy, from −2 % to 8.2 %. Raw:
    `/work/.al-sem/profiles/2026-11-17-eng125/e125-proto.log`.

### Task 1: `multi_walk` and the event model

**Files:**
- Modify: `src/program/impact_graph/reverse.rs`
- Create: `src/program/impact_graph/multi_walk_tests.rs`

**Interfaces:**
- Produces:
  ```rust
  pub struct RootBits(pub Box<[u64]>);
  pub trait MultiSink {
      fn reach(&mut self, s: WalkState, hops: u32, via: Via, from: Option<WalkState>, roots: &RootBits);
      fn hidden(&mut self, n: NodeIx, ch: HiddenChannel, roots: &RootBits);
      fn entry(&mut self, n: NodeIx, kinds: &[EntryKind], s: WalkState, hops: u32, roots: &RootBits);
      fn external(&mut self, n: NodeIx, why: ExternalWhy, s: WalkState, hops: u32, roots: &RootBits);
      fn source(&mut self, s: WalkState, hops: u32, roots: &RootBits);
      fn step(&mut self, from: WalkState, site: SiteIx, roots: &RootBits) -> u8; // pure in (from, site)
      fn rejoin(&mut self, n: WalkState, hops: u32, site: SiteIx, roots: &RootBits);
      fn limit(&mut self, root: usize, l: Limit);
  }
  /// `roots[i]` = logical root i's lanes (its nodes, in order).
  pub fn multi_walk(g: &SitedGraph, roots: &[Vec<NodeIx>], opts: &WalkOptions<'_>, sink: &mut dyn MultiSink) -> Vec<WalkStats>;
  ```

- [ ] **Step 1: Write the tests**
  - `multi_walk_traces_equal_per_root_walks`: a recording sink, demultiplexed by bit. For every
    logical root, the full callback trace (every method above, with arguments) equals a recording
    `ConeVisitor`'s trace from `walk_multi` on that root's nodes. The graphs:
    - the `World` graphs of `up_tests.rs` and `reverse_tests.rs`;
    - new `World` fixtures: two roots reaching one state from different parents; a two-arm root
      with unequal distances; the universal hub (with an already-reached hub); a table-event
      guard; action-run sites (a page reached twice);
    - `tests/fixtures/test-universe`'s sited graph.
  - `multi_walk_budget_boundaries`: budgets 0, exact and exact + 1, on a fixture with two
    roots. Trace equality, including the last state's steps.
- [ ] **Step 2:** Run `cargo test -p al-sem --lib multi_walk`. Expected: FAIL.
- [ ] **Step 3: Implement**
  - Intern states in an FxHashMap, with `seen: RootBits` per state (shared by a root's lanes).
  - A level is `Vec<(lane, child, from, site, new_bits)>`, stably sorted by (lane, `WalkState`).
    Each (lane, child) group: reach and report per record, in arrival order. Then ONE site loop
    for the group, with the OR of its records' bits (each root's sequence is unchanged, and the
    site loop is not repeated per record).
  - Record on CDO: the peak records per level, and the peak bytes of the level vectors.
  - Per site: apply the open, guard and step rules; the action-run entry for
    `bits & !action_entries[child]`; `rejoin` for `bits & seen[child]`; then
    `new = bits & !seen[child]`, set them, and push a record.
  - Budget and cancellation as §Event model says.
- [ ] **Step 4:** Run the tests. Expected: PASS.
- [ ] **Step 5: Discrimination proofs, each red then green**
  - The level keyed by `state_ix`. Assert first, on the fixture, that intern order ≠ `WalkState`
    order.
  - One record per child (provenance dropped).
  - Lanes blocking only higher lanes.
  - `rejoin` bits taken after `seen` is updated.
  - `limit` fired at the budget instead of past it.
- [ ] **Step 6:** Commit.

### Task 2: `UpMulti` pass 1, and the go/no-go

**Files:**
- Create: `src/engine/impact/up_multi.rs`, `src/engine/impact/up_multi_tests.rs`
- Modify: `src/engine/impact/up.rs` (`node_json`, `witness_json`, `by_relation`, `share_json`,
  `display`, `callers_json` become `pub(super)`)

**Interfaces:**
- Consumes: Task 1.
- Produces: `pub(super) struct UpMulti<'g>`:
  - `fn new(g, keep, roots: &[Vec<NodeIx>], union: bool, shared) -> Self`;
  - the §Memory bound masks, sparse by reached node and entry;
  - `fn rows_p1(&self) -> Vec<P1Row>`.

- [ ] **Step 1:** Write the P1 tests (every P1 row of the table), comparing each field's JSON
  with the oracle's, with Review Focus 5's fixture.
- [ ] **Step 2:** Run them. Expected: FAIL.
- [ ] **Step 3:** Implement the sink and the folds. A fold iterates reached nodes (or entries) by
  words and counts each newly best root bit. Apply the per-field root and hub exclusions from the
  table.
- [ ] **Step 4:** Run the tests. Expected: PASS.
- [ ] **Step 5:** Discrimination proofs, each red then green:
  - the highest tier taken as best;
  - population-only class masks (Review Focus 5's fixture);
  - external dropping root nodes;
  - hub `at` taken as a numeric minimum.
- [ ] **Step 6: Go/no-go, in the production chunk layout.**
  - On CDO, in the slot, time `multi_walk` plus pass 1 for the 699 roots and the union, at 1, 2 and
    8 chunks. Report CPU summed over chunks, wall time, and the up pass's added peak RSS.
  - Choose the fewest chunks whose wall time is within cdo-speed's limit.
  - **Stop and report** if, at that chunk count:
    - the per-root pass's CPU minus the estimated two-pass cost is under 5 % of impact's CPU. The
      estimate is 2 × this time and is labelled an estimate: pass 2 has slot work pass 1 lacks,
      and it is re-checked after Task 3; or
    - the added peak exceeds about 350 MiB.
- [ ] **Step 6b:** A lib test with more than 64 roots (`tests/fixtures/test-universe`, every
  routine a root), run at chunk sizes 1, 64 and all. Every row is equal across the chunkings and
  to the oracle.
- [ ] **Step 7:** Commit.

### Task 3: `UpMulti` pass 2, the witness slots

**Files:** Modify `src/engine/impact/up_multi.rs` and `src/engine/impact/up_multi_tests.rs`.

**Interfaces:**
- Consumes: P1's final masks.
- Produces:
  - `fn pass2(&mut self) -> impl MultiSink + '_`;
  - `Slot<K: Ord>`: insert (key, event order); `close_after(level)`;
  - `fn rows(&self) -> Vec<Value>` (the full rows).

- [ ] **Step 1:** Write the P2 tests (every P2 row), with Review Focus 1's fixture, a late
  non-open upgrade, a caption chosen before the page enters a list, and equal-key ties.
- [ ] **Step 2:** Run them. Expected: FAIL.
- [ ] **Step 3:** Implement the slots with the real keys. A slot stays open until the level
  boundary after its worst member's hops + 1. Only the bits whose slot is open are visited per
  event.
- [ ] **Step 4:** Run the tests. Expected: PASS.
- [ ] **Step 5:** Discrimination proofs, each red then green:
  - first-fill without the key (Review Focus 1);
  - closing at the member's own level;
  - arrival pop used instead of P1's final pop;
  - one class slot per (word, pop) with the rank in the key (the rank-upgrade fixture);
  - an open-only member relabelled non-open (the open-then-non-open fixture);
  - the delivered bit set at the first visit, with a fixture where a publisher's `rejoin` at h+1
    comes before its `reach` at h in the same level.
- [ ] **Step 6:** Re-check the bar: pass 1 plus pass 2's measured CPU at the chosen chunk count,
  against the per-root pass. Stop and report if the saving is under 5 %.
- [ ] **Step 7:** Commit.

### Task 4: Wire into `up()`: chunks, the union, the oracle

**Files:**
- Modify: `src/engine/impact/up.rs`, `src/engine/impact/up_par_tests.rs`
- Test: `up_multi_tests.rs`, and a lib CDO test `up_multi_cdo_rows_equal_oracle` (`cdo` in the
  name, through `cdo_ws_or_enforce`)

- [ ] **Step 1: Tests**
  - `multi_rows_equal_oracle_rows` and `multi_union_equals_oracle_union` on every fixture.
  - The lib CDO test: the pinned baseline's 699 rows and the union, byte-equal (cost fields
    blanked) between the oracle and production.
- [ ] **Step 2: Replace the per-root pass and the union walk**
  - Split the roots into the chunk count Task 2 chose, on the reach pool (serial in a resident
    revision, as now).
  - Each chunk runs `UpMulti` P1 → P2 → rows. The union runs as one logical root in its own
    chunk.
  - `walk_root`, `Up`, `summary` and `classified` move under `#[cfg(test)]`.
  - Adapt `up_par_tests.rs`' worker probes to chunks.
  - Keep the `impact.up.*` spans and counters, and add `impact.up.multi_ms`.
- [ ] **Step 3:** Run `scripts/check-goldens` and `cargo test -p al-sem --lib impact`. Expected:
  every impact golden unchanged.
- [ ] **Step 4:** Commit.

### Task 5: Measure and gate

- [ ] **Step 1:** `cdo-before-after <master> <head>`: nothing moved. Six or more timed impact runs,
  their outputs byte-identical (sha256 of each, cost fields blanked).
- [ ] **Step 2:** 3+ alternating clean rounds; claim a CPU ratio only if the pairs agree. Also
  report wall time and peak RSS.
- [ ] **Step 3:** `cdo-speed` twice. `cdo-gate --only harness,l4,lib,output` (the lib CDO test
  included).
- [ ] **Step 4:** CHANGELOG; Fable, then sol; the task gate; submit.
