# #160 child spec: step 7a — one inherited set per Cone-SCC

Status: revised after spec panel round 1 (2026-10-20; findings S1-1 to S1-23 in
`.agent/issue-160/findings.json`).
Child of `2026-10-18-issue-160-analysis-session-design.md` (the epic): §5.6, §9 row
**7a**, E2, E10, E14, E15, Decisions 32, 34, 43, 44, 54, open question 2. Follows the
structure of `2026-10-20-issue-160-step4-exec-relation-design.md` ("child 4" below).
Where this document and the epic disagree, this document is wrong, EXCEPT the numbered
amendments of §9 (7A-1 to 7A-4), each with its reason; until the epic takes them, they
are open questions for the panel.

Base: master `d37e8634` (step 4 merged at `c186f4d1`, then three `.kraken` commits that
touch no `src/`, `tests/` or `crates/` file), worktree `/work/al-sem-issue-160-a1`.
Every `file:line` is in that tree and was read before it was cited. Every fact the design
rests on is a row of the ledger's `## Assumption probes` table
(`.agent/issue-160/ledger.md`, P1-P24).

Words:
- **Cone graph** = the typed-edge graph the capability cone builds from `rel.cone()`
  (`build_typed_edge_graph`, `capability_cone.rs:2835`). **Cone-SCC** = an SCC of that
  graph, from the cone's own Tarjan (`capability_cone.rs:2844-2849`). It is NOT
  `rel.scc()`, which is the SCC of the Primary payload (`exec_relation.rs:83-88`).
- **Recursive SCC** = a Cone-SCC with more than one member, or one member with a
  self-edge (`Scc::recursive`). Only recursive SCCs take the BFS path
  (`capability_cone.rs:2588`).
- **Derived row** = one routine's row of `ConeDerivedStore` (`cone_derived.rs:390-438`):
  presence flags plus four id sets. **F(X)** = the row the builder folds from a set of
  facts X (`fold_fact`, `cone_derived.rs:725-772`, then `finish_routine`, `:774-799`).
- **Own entries of m** = the producer-solve entries resolved in routine m's frame
  (`own_entries(m)`, `capability_cone.rs:2484`).
- **Self half of m** = m's raw direct facts, folded into m's row after the walk
  (`capability_cone.rs:2641-2656`).
- **SCC set of i** = the fact cone of Cone-SCC i (`fact_cone_for_scc`,
  `capability_cone.rs:2140-2291`): every member's key-deduped direct facts at distance
  0, plus every successor SCC's cone entries at distance + 1.

## 1. Goal

For a member of a recursive Cone-SCC, the derived row is folded from **one set per
SCC**, not from a breadth-first walk per member. Today each of the 714 recursive members
on CDO cross-app `analyze` runs its own walk over the whole SCC and pulls every successor
cone (`inherited_facts_by_bfs`, `capability_cone.rs:1979-2088`); the largest SCC has 479
members, so the same work is repeated up to 479 times. The walk is the `bfs.walk` timer:
8,627.5 ms median on CDO `analyze`, band 223.6 ms
(`docs/measurements/2026-10-18-issue-160-step0.md`, §4.1.1 table).

After 7a, under `ConeOutput::DerivedOnly` the cone runs no per-member walk. Each
recursive member's row is F(SCC set) ∪ F(own entries of m) ∪ F(self half of m), which
equals today's row exactly (§4.1). No output byte moves (epic I2).

## 2. Non-goals

- **No `fold_row`, no interner, no hash-consed sets, no fold memo.** Those are step 7b
  (epic §9 row 7b). 7a's per-member fold stays one `fold_fact` per fact (§4.4, D7a-2).
- **The singleton path is unchanged** (`inherited_facts_for_singleton`,
  `capability_cone.rs:1741-1958`): its scan and fold (`scan_cone` 2,669 ms, `fold`
  1,810 ms on CDO) are 7b's target.
- **The raw path is unchanged.** `RawOnly` and the raw half of `Both` still walk each
  recursive member. The `RawOnly` callers: r3a5 (`compose_cone_over_graph`,
  `capability_cone.rs:3840-3846`), the snapshot (`compose_cone_over_graph`, `:4279-4285`)
  and `project_r3a3`, which calls `compose_inherited_cones` DIRECTLY
  (`:4097-4107`). The `Both` caller: policy's RAW build (`detector_context.rs:1202`;
  `src/engine/gate/policy/pipeline.rs:258`). They still walk each recursive
  member: the raw inherited facts carry a per-member witness (first hop, distance), which
  a per-SCC set cannot give. `inherited_facts_by_bfs` stays production code.
- **The Cone adjacency and SCC do not join `ExecRelation`** (M3 decided here, §4.6).
- **No change to `pd_solve`**, to the coverage cone, or to which routines get a row.
- **No new peak bound.** The epic lists no peak bound for 7a (§5.12). Peaks are reported
  only, and only where the platform can read them (§8 part 3).

## 3. Background: what the walk does today

`compose_inherited_cones` (`capability_cone.rs:2416-2777`) visits Cone-SCCs callees
first. For SCC i it builds the fact cone, unless i is a root (nothing calls it,
`:2525-2550`), then emits a row for each member (`:2575-2683`). A recursive member m
runs `inherited_facts_by_bfs` (`:2588-2604`, timed as `BFS_WALK_NANOS`), which:

1. marks m visited (`:1991-1992`), so m's OWN direct facts are never picked;
2. expands every sibling it reaches and picks each sibling's key-deduped direct facts,
   first key wins (`:2016-2035`);
3. at a node of another SCC, pulls that SCC's whole fact cone with base keys (guards
   dropped) and does not recurse (`:2036-2066`);
4. lets m's own entries compete per key (`:2069-2083`).

Then `emit_picks` folds every pick (`:2093-2113`, timed as `BFS_FOLD_NANOS`), and the
self half folds m's raw direct facts (`:2641-2650`).

The census numbers for CDO cross-app `analyze` (step 0 numbers file, §4.1.1): BFS calls
714, `BFS_PICKS` 4,344,888 (about 6,085 picks per member), `max_scc_members` 479,
`bfs_walk` 8,627.5 ms (band 223.6), `bfs_fold` 1,029.3 ms (band 31.8). The 479-member
SCC is a Base Application hub that issue #96's implicit-trigger edges joined
(`.agent/issue-96/ledger.md:96`: Gen. Journal Line `OnInsert` → approvals → workflow
`ExecuteResponse` → posting).

## 4. Design

### 4.1 Why one set per SCC gives the same row

Claim: for a member m of recursive Cone-SCC i,

    old(m) = F(picks(m)) ∪ F(self half of m)
    new(m) = F(SCC set of i) ∪ F(own entries of m) ∪ F(self half of m)
    old(m) = new(m)

Each step, with its source:

1. **F is a function of the key.** `fold_fact` reads only `resource_kind`, `op`,
   `resource_id` and `fact_is_known_temp` (`cone_derived.rs:725-772`), and all four are
   in `inherited_fact_key` (op | kind | resource id | confidence | temp class). Two facts
   with one key fold the same (`cone_derived.rs:51-66`, issue 33). So which
   representative a key keeps (first-seen in the BFS, min `rep_key` in the cone) does
   not matter.
2. **F is a union.** Flags are OR-ed; ids are pushed, then sorted and deduped at freeze
   (`freeze_ids`, `freeze_masked`, `cone_derived.rs:831-860`). Folding a key twice is a
   no-op. So F(A ∪ B) = F(A) ∪ F(B) for any fact sets.
3. **The walk reaches every sibling and every successor SCC.** A path between two
   members of one SCC stays inside it, so the walk from m reaches every sibling s ≠ m
   and expands each one's out-edges (`:2026-2035`), and m's own out-edges are the seed
   (`:2003-2012`). So every edge from the SCC to another SCC is seen, and that SCC's
   cone is pulled. `build_succ_sccs` collects exactly these SCCs (`:2117-2136`).
4. **The SCC set holds the same keys plus m's own direct keys.** For a recursive SCC the
   fact cone has no guard anchor (`:2528-2538`: only a non-recursive singleton gets one),
   so every member's direct facts enter with no guard (`:2164-2207`) and every successor
   entry enters with its base key and `guard: None` (`:2270-2287`; the per-edge carry
   at `:2216-2253` runs only with an anchor). The walk also drops guards
   (`:2054-2056`). So keys(SCC set) = keys(walk picks before own entries) ∪
   keys(direct(m)).
5. **m's own direct keys are already in the self half.** `direct(m)` is the key-deduped
   copy of m's raw direct facts minus producers (`dedup_direct`, `:1557-1569`); the self
   half folds every raw direct fact of m. So F(direct(m)) ⊆ F(self half of m).
6. **Own entries are per member and stay per member.** The walk's own-entry merge
   (step 4 above) adds exactly keys(own entries of m). `new(m)` folds them for m alone.
7. **Producers never enter the SCC set.** `dedup_direct` drops them (`:1559`) and the
   cone holds no producer (`pd_solve.rs:1-19`). A non-producer fact keeps its rep and
   temp state on every edge (`substitute_entry` copies `rep` unchanged,
   `capability_cone.rs:1586-1612`). This answers the epic's open question 2: yes, the
   temp class of a non-parameter-dependent fact is constant along a path.

Where members differ: only in (6) own entries and (5) the self half. Guards do not make
members differ inside a recursive SCC (point 4). No per-member filter exists in the walk
except `routine_ids.contains(m)` (`:2576`), which gates emission, not membership.

### 4.2 The new recursive branch

In `compose_inherited_cones`, for each recursive SCC i with at least one member in
`routine_ids`, when `mode.wants_derived()`:

1. **Get the SCC set** once:
   - not a root: borrow `fact_cones[i]`, already built at `:2539-2541` and not freed
     until after the member loop (`:2686-2694`);
   - a root (`remaining_uses[i] == 0`, `:2525`): build it with the same call,
     `fact_cone_for_scc(&members, succ_ids, &fact_cones, direct, None)`, keep it in a
     local, and drop it after the member loop. It is never inserted into `fact_cones`,
     so the leak guard at `:2703-2708` is unchanged.
   Timed under `BFS_WALK_NANOS` (§4.5).
2. **Per member m**, between `begin_routine` (`:2581`) and the self half (`:2646`):
   `fold_fact(&entry.rep)` for every entry of the SCC set, then for every own entry of
   m. Timed under `BFS_FOLD_NANOS`.
3. **When `mode.wants_raw()`** (RawOnly, Both): `inherited_facts_by_bfs` runs as today
   and `emit_picks` builds the raw `Vec` only. Its fold half (`:2099-2103`) is deleted
   from `emit_picks`, and so is its `derived` parameter: `emit_picks(subject, picks,
   mode)` keeps no argument that a non-test build leaves unused (clippy `-D warnings`).
   The oracle's fold of the picks is a loop at the call site, inside the
   `cfg(test)`-only `PerMemberWalk` arm (§4.3). In production the derived row always
   comes from step 2, whatever the mode. So `Both` folds from the SCC set too, and
   policy's RAW build gets the same derived rows as the det cone.

`DerivedOnly` therefore calls `inherited_facts_by_bfs` zero times. The singleton
branch (`:2612-2625`) is untouched.

### 4.3 The old path, test-only until CDO passes

```rust
/// How a recursive member's derived row is folded (#160 step 7a).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RecursiveMembership {
    /// One set per Cone-SCC (§4.2). The only production value.
    PerScc,
    /// The pre-7a path: fold the member's own BFS picks. The oracle.
    #[cfg(test)]
    PerMemberWalk,
}
```

`compose_cone_over_graph` (`capability_cone.rs:2825-2832`) keeps its signature and calls
`compose_cone_over_graph_with(.., mode, RecursiveMembership::PerScc)`, a `pub(crate)` fn
that takes the extra argument and passes it to `compose_inherited_cones`, which gains
the same parameter. Its other direct caller, `project_r3a3` (`capability_cone.rs:4097`),
passes `RecursiveMembership::PerScc`; under its `RawOnly` mode the parameter changes
nothing, since no derived row is folded. Under `PerMemberWalk` the recursive branch runs
today's code exactly: the walk, then a fold of every pick (today's `emit_picks` fold
half, `:2099-2103`, moved to the call site inside the `cfg(test)` arm), then the raw
emission. Because the
variant exists only under `cfg(test)`, no production build can select the old path:
"test-only" is enforced by the type, not by a comment.

**Deleted when:** the old path is 7b's oracle too (epic §9 row 7b, "7a's oracle"). The
variant and its branch are deleted by step 7b's last commit, after 7b's own CDO oracle
passes. If 7b is dropped under its own rule (X < 2 × band), the variant is deleted by the
first later step that edits `compose_inherited_cones`, step 6 at the latest (it rewrites
the witness engine, epic §5.7). Until then the CDO oracle (§5.8) runs in every
`scripts/cdo-gate`.

### 4.4 The fold half stays per member (D7a-2: `fold_row` is NOT in 7a)

Epic row 7a requires this spec to say whether `fold_row` moves into 7a. **It does not.**
Per member, 7a folds every entry of the SCC set with `fold_fact`: about the same number
of calls as today's picks, so `bfs.fold` stays about where it is (1,029 ms on step 0).
Reasons:

1. **The walk is the cost.** On step 0, `bfs_walk` is 8,628 ms of the 9,657 ms the
   recursive branch spends (89 %). The walk's cost is the queue and the `seen` map,
   which allocates one `String` per newly seen key (`:2061-2063`), not the fold.
2. **`fold_row` is step 7b's primitive.** 7b brings the interner, the set arena and the
   per-set memo; `fold_row` folds a memoized set's row. A narrower "seed the scratch
   from an SCC row" in 7a would be a second fold primitive that 7b then replaces, and
   7b's memo discrimination ("disable the memo → two folds") would have to account for it.
3. **The bound stays clean.** With the fold outside 7a, the `bfs.walk` timer measures
   only what 7a removes, and `bfs.fold` is reported, not bounded (§8 part 2).

Consequence, stated: 7a's expected cone saving is the walk alone (≈ 8.6 s on step 0's
machine), not walk + fold.

### 4.5 Timers and counters

Census timers keep their names, so step 0's lines stay comparable:

- `BFS_WALK_NANOS` (`detector_context.rs:378`) covers, at head: building the SCC set of
  a ROOT recursive SCC, borrowing it for a non-root one, and the per-member walk when the
  raw half needs it (RawOnly, Both). In `analyze` (DerivedOnly) only the first two.
- `BFS_FOLD_NANOS` (`:379`) keeps the parent's coverage: it wraps everything after the
  walk for a recursive member. On the parent that is all of `emit_picks`
  (`:2605-2611`): the fold, plus retag and sort when the mode wants raw. At head it
  wraps the per-member fold of §4.2 step 2 AND, when the mode wants raw, `emit_picks`
  (retag and sort). In `analyze` (DerivedOnly) both sides time the fold alone, so the
  `analyze` reading is like for like; in raw modes too, since both wrap the same work
  plus the fold.
- `BFS_CALLS` (`:381`) keeps counting recursive member EMISSIONS (after the
  `routine_ids` filter, `:2576`; 714 on CDO).
- `BFS_PICKS` (`:380`) counts picks of walks that actually ran: 0 in DerivedOnly at head.

**The overlap rule.** `COMPOSE_REST_NANOS` = the compose call's elapsed time minus the
eight component timers (`detector_context.rs:474-486`), and
`census_rest_is_never_negative` (`tests/cli/cli_build_counters.rs:733-756`) fails on a
negative value. So every new timing is measured with its own `Instant` and is either
(a) added to exactly ONE component (the root-set build and borrow to `BFS_WALK_NANOS`
only, the set fold to `BFS_FOLD_NANOS` only, never also to `FACTCONE_NANOS`, though the
root-set build calls `fact_cone_for_scc`), or (b) a REPORT value that is not a component
and is not subtracted (`walk_total`, `root_set_probe`, below). No timed region nests
inside another component's region.

**Census counters that move at head** (process totals, `analyze` cross-app):
- `BFS_PICKS` 4,344,888 → 0 (no walk runs in DerivedOnly);
- `MERGE_CALLS` / `MERGE_TIEBREAKS` do not fall (the walk never calls `merge_cone`);
  they RISE by the merges of the root-set builds (`fact_cone_for_scc`
  merges, `:2197`, `:2275`), the same number as Task 1's `root_set_probe` performs;
- `bfs_walk` and `bfs_fold` timings change (the point of the step);
- unchanged: `BFS_CALLS`, `FACTCONE_CALLS` (the root-set build is not counted there:
  that counter is at the non-root call site, `:2546-2549`), `max_scc_members`, every
  singleton and `pd_solve` counter, `derived_fold`.

New, added by plan Task 1 BEFORE the switch (the stop rule's instrument, §6). Task 1
changes no output and no production path:

- A per-call `ComposeStats` struct, plain integers, computed always and returned on
  `ConeOutcome` as a `pub(crate)` field (so tests read it with no global state):
  `recursive_sccs`, `recursive_members_all` (every member Tarjan placed in a recursive
  SCC, emitted or not), `recursive_members_emitted` (those that pass `routine_ids`, the
  population `BFS_CALLS` counts), `recursive_roots`, `self_loops`,
  `max_recursive_members`, `size_hist: [u64; 5]` (members 1, 2, 3-9, 10-99, ≥ 100),
  `member_walks` (calls of `inherited_facts_by_bfs`), `own_entries_recursive` (own
  entries of emitted recursive members, which the new path folds one each),
  `scc_sets_borrowed`, `scc_sets_built`, `scc_set_entries_folded`.
- Census atomics fed from it once per call, and, under `ALSEM_CONES_CENSUS=1` only:
  `walk_total` (Σ walk ns over recursive members, read from the same `Instant`s as
  `BFS_WALK_NANOS`, a report) and **`root_set_probe`**: at Task 1, census ON only, for
  every ROOT recursive SCC, build its set with the exact call §4.2 step 1 will use,
  count its entries into `root_set_entries`, time it, and drop it. A report, not a
  component (the overlap rule); it adds to `rest=`, which stays non-negative. This
  measures root-set construction directly; it is not a projection. The probe is
  deleted by the switch commit, where the same build becomes production and is timed
  under `BFS_WALK_NANOS`.
- One new census line, printed after the `compose:` line
  (`detector_context.rs:566`):
  `[cones-census]   cone_scc: recursive={} members_all={} members_emitted={} roots={} self_loops={} max={} sizes=1:{} 2:{} 3-9:{} 10-99:{} 100+:{} member_walks={} own_entries={} sets_borrowed={} sets_built={} set_entries={} walk_total={:.1}ms root_set_probe={:.1}ms root_set_entries={}`.
  Census output goes to stderr only when the census is on; no test pins its text today
  (grep for `bfs_walk`/`picks=` in `tests/` finds nothing).

The perf-trace build counters (`build.cone`, `build.graph_scc`, …) do not move (I1): 7a
adds no graph, no Tarjan and no cone. `cli_build_counters` stays green unchanged.

**Transient memory.** A root SCC's set lives from its build until its last member is
emitted: one SCC's fact cone at a time, never two. Its size is `root_set_entries` (Task
1 reads it; no committed number exists). The non-root sets are the fact cones the walk
already holds today (`:2539-2541`). 7a has no peak bound (§2); the root set's size is
recorded in the numbers file.

### 4.6 M3 decided: the Cone pair stays inside the cone until step 6

Child 4's amendment M3 (epic §5.5) says the Cone adjacency and its SCC join
`ExecRelation` at step 6 or 7a, "whichever spec lands it first", that is at their first
second permanent reader. **7a does not land it.** Reasons:

1. **7a adds no second reader.** The SCC set is built inside `compose_inherited_cones`
   from the SCC that `compose_cone_over_graph` already computes (`:2844-2849`). The only
   other reader 7a adds is the test-only oracle (§4.3), which composes its own.
2. **7a cannot reuse `rel.scc()`.** That is the Primary SCC (`exec_relation.rs:83-88`):
   on CDO it has no SCC larger than one member (step 0 numbers file, "SCC
   distribution"), while the Cone graph's largest has 479. They are different graphs:
   the Cone payload adds the dependency callers' typed edges (child 4 §4.2).
3. **Holding the pair in the relation lengthens its life with no reader to pay for
   it.** The typed graph and the Cone SCC would live from the det cone through every
   run (child 4 §9 M3), and every view that builds no cone (query; `analyze --detector
   d47-…`) would run one Tarjan more. (These are relation bytes, `rel.*` buckets, not
   the `context.cone_derived` + `context.summaries` buckets A7a.6 bounds; the argument
   is lifetime, not that bound.)
4. **Step 6 brings the reader.** Step 6 moves the snapshot onto `Transfer::Cone`
   (epic §9 row 6), the pair's second permanent reader; policy's duplicate pair (child 4
   §4.9, policy `scc` 3) ends there too, when the RAW build is deleted (Decision 16).

So the pair joins the relation at step 6; the epic's M3 text "(or 7a, whichever spec
lands it first)" resolves to step 6 (amendment 7A-4).

### 4.7 Order and determinism (I7)

- Raw outputs (RawOnly, the raw half of Both) are unchanged: same walk, same
  `sort_inherited` order.
- The derived store's rows are equal by content, but the **interning order** of resource
  ids changes, because 7a folds the SCC set in its `BTreeMap` key order instead of the
  walk's emission order. Interning order never reaches output: every accessor resolves
  ids and sorts by string (`cone_derived.rs:499-646`), `transaction_spans` sorts the
  resolved ids (`transaction_spans.rs:251-262`), `pending_writes` collects into a
  `BTreeSet` (`pending_writes.rs:57`, `:191-196`). The store has no `PartialEq` and
  nothing compares `ResId`s across stores. The oracle therefore compares rows by
  resolved content, never by pool bytes (§5.1).
- The census's `context.cone_derived` bytes count pool lengths and interned string
  bytes (`detector_context.rs:1711-1720`), which do not depend on the order: equal by
  construction (A7a.6, §8 part 1).

## 5. Test plan

Every new test lists its break, made with Edit, run, reverted with Edit, both outcomes
recorded in the ledger (CLAUDE.md "Testing Philosophy"). **How a proof shows the break
applied:** the Edit tool refuses an `old_string` that does not occur exactly once, so a
break that matches nothing cannot run silently; and the red run must fail AT the
assertion the break names (the proof records that assertion's message), not elsewhere.
Each test also asserts its own PRECONDITION from the composition's output before its
main assertions, so a fixture that stops producing the shape fails loudly instead of
passing vacuously. All are `--lib` tests in `capability_cone::tests` unless named
otherwise.

### 5.1 The row comparator

`fn assert_rows_equal(label, a: &ConeDerivedStore, b: &ConeDerivedStore)`: the two
stores' row-id sets are equal, then for every id: equal `flags_of`, `writes_tables_of`,
`physical_table_write_ops_of`, `writes_physical_tables_of`, `physical_table_reads_of`,
`publishes_events_of`. It never compares `ResId`s. The accessors return the same empty
row for an absent id and for an empty row (`EMPTY_ROW`, `cone_derived.rs:408-414`), and
`rows` is private (`:425-432`), so 7a adds a `#[cfg(test)] pub(crate) fn row_ids(&self)
-> BTreeSet<&str>` on `ConeDerivedStore`. Its own guard, `rows_comparator_sees_each_field`:
seven hand-built pairs, each differing in one thing: flags, writes-all, physical writes,
an op mask, physical reads, events, and **a pair whose second store has one extra
EMPTY row for an id outside the first** (built with `begin_routine`/`finish_routine`
and no fold). It asserts the comparator panics on each (`catch_unwind`, and the message
names the field or "row ids").

### 5.2 Hand-stated rows and raw output (T1, `scc_set_rows_hand_stated`)

Graph (synthetic `TypedEdge`s): A ↔ B (one SCC), B → C; A writes table `t/A`, B
publishes event `e/B`, C writes `t/C`. Precondition: `stats.recursive_sccs == 1`.
Expected derived rows, literal: A and B carry `t/A`, `t/C`, `e/B` and the table and
physical-table flags; C carries `t/C`. Asserted under DerivedOnly AND equal to the
oracle (`PerMemberWalk`).
**Break:** skip the successor merge in `fact_cone_for_scc` (`:2254-2289`) when the SCC
is recursive → the SCC set lacks `t/C` → A's literal row fails on `t/C`. (The oracle's
walk pulls C's cone directly, `:2051-2053`, so only the new path changes.)

### 5.3 Root and self-loop SCCs (T2, `scc_set_root_and_self_loop`)

Two shapes in one graph: a ROOT two-member SCC {X, Y} (nothing calls it) with X → S,
S writing `t/S`; and a non-root self-loop R (R → R, R → S2, S2 writing `t/S2`) called
by a singleton U. Precondition: `stats.recursive_roots == 1`, `self_loops == 1`.
Literal rows (X and Y carry `t/S`; R and U carry `t/S2`), and equal to the oracle;
`stats.scc_sets_built == 1`, `scc_sets_borrowed == 1`.
**Break:** for a root SCC use an empty set instead of building one → Y (which reaches
`t/S` only through X) loses `t/S`; Y's literal row fails.

### 5.4 Own entries differ per member (T3, `scc_set_keeps_own_entries_per_member`)

Why a guard is needed: a known temp state crosses every caller edge unchanged
(`cross_edge`, `pd_solve.rs:204-232`), so in a cycle every member ends with the same
own-entry keys unless some edge CONTRADICTS a requirement (`carry_reqs` →
`across_edge` returns `None` on a literal that contradicts, `param_guard.rs:246-277`).
The inherited key has only two temp classes, `kt` and `nt`
(`capability_cone.rs:1305-1314`). So the fixture differs the members by a contradicted
guard. Inline AL through the `model(SRC)` helper (`capability_cone.rs:5765-5774`),
which declares exactly one table, `table 50100 Cust`; the fixture writes that table:

```al
codeunit 50100 C {
  procedure W(Flag: Boolean; var R: Record Cust) begin if Flag then R.Insert(); end;
  procedure Q(Flag: Boolean) var R: Record Cust; begin W(Flag, R); P(); end;
  procedure P() begin Q(false); end;
}
```

**Table names in every §5 fixture.** The inline-AL tests use only tables the model they
build declares: T3 and T4 use `model(SRC)` and table `Cust`; T3b needs eight distinct
tables, so it uses a new test helper `model_with_tables(names, src)` beside `model`: it
emits one table file per name, in the exact text shape of `model`'s Cust table, with
ids 50101.., and calls the same `assemble_and_resolve_inline_program_default`. T1, T2,
T5, T6, T9, T10 and T11 are synthetic `TypedEdge` graphs with hand-built facts: their
`t/…` ids are plain strings that no model resolves, so they need no table. Below,
"[T]" and "[T1..T8]" mean the resolved resource id the model gives that declared table,
read from the model's table row for that name (a fixture input), never from the code
under test.

Trace, by hand: `R.Insert()` in W is a producer (its record is parameter 1) under
`Flag = true` (W's `Flag` is a usable guard: by-value Boolean, never assigned, `frame_guards`,
`param_guard.rs:69-98`). Across Q → W: the temp state at the call site is physical (Q's local `R`), and
the requirement maps to Q's own `Flag` (Q's frame has it usable, `across_edge`'s
parameter arm, `param_guard.rs:262-273`), so Q holds one state, physical, with
`[Q.Flag = true]`. Across P → Q the argument is the literal `false`, which contradicts
→ `None` (`param_guard.rs:257-261`): P holds no state for this producer. P and Q form
one SCC (P → Q, Q → P). W is a separate non-recursive SCC whose cone holds no producer
(`dedup_direct`, `capability_cone.rs:1559`), so the SCC set carries no Cust write at all.
**Precondition asserted** from `outcome.pd_solution.by_routine` (`pd_solve.rs` arena):
Q has ≥ 1 non-seed state, P has 0. Expected rows: `writes_physical_tables_of(Q) ==
[T]` with T = Cust's id, `writes_physical_tables_of(P) == []`. Equal to the oracle.
**Break 1:** fold the FIRST member's own entries for every member (index
`own_entries(&members[0])` instead of `own_entries(m)` in the new fold) → whichever of
P, Q is first, the other's row flips (P gains T, or Q loses it); the literal row
assertion fails. **Break 2:** skip the own-entry fold → Q loses T (the set has none);
fails.

**T3b, producer blow-up (`scc_set_own_entries_scale`).** The count version, with the
callers INSIDE the cycle, over `model_with_tables(["T1", …, "T8"], src)`: eight
procedures M1 → M2 → … → M8 → M1, and Mi calls `Wi(var R: Record Ti)` with a local
physical record; each `Wi` inserts into its parameter (eight producers, no guard). Each
Mi also calls `Commit()`, one direct non-producer fact per member (`commit` /
`transaction`, no resource id), so the SCC set is NOT empty: all eight share one key,
so the set holds exactly one entry. Hand trace: each producer's physical state at Mi
crosses every caller edge unchanged, so every member holds exactly one state per
producer. **Precondition asserted:** each Mi has exactly 8 non-seed states in
`by_routine`; `stats.own_entries_recursive == 64`; the set has 1 entry
(`stats.scc_set_entries_folded == 8`, eight members × one entry, a non-zero count); every
Mi's flags carry `MAY_COMMIT`. Rows: every Mi's `writes_physical_tables_of == [T1..T8]`;
equal to the oracle. **Break:** skip the own-entry fold → every row loses T1..T8
(producers never enter the set; the set's one entry is the commit, which writes no
table) → the rows assertion fails, while the `MAY_COMMIT` and set-count assertions still
hold, which shows the break hit the own entries and nothing else.

### 5.5 Guarded successors (T4, `scc_set_carries_guarded_successor_entries`)

Inline AL through `model(SRC)` (table `Cust`, the only one it declares): recursive SCC
{A, B} (A → B, B → A); B calls `G(true)`; G is
`procedure G(Flag: Boolean) begin if Flag then Rec2.Modify(); end` over a GLOBAL
physical record `Rec2: Record Cust` (not a producer: its temp state is known, not
parameter-dependent). Below, "the T2 write" means this Modify of Cust. Also two non-recursive callers outside the SCC: `N()` calls
`G(false)`, `M()` calls `G(true)`.
Hand trace: G is a non-recursive singleton with a guard frame, so its fact cone mints
the write with a requirement → a frame-bound (guarded) entry (`:2154-2196`,
`frame_bound`, `:1537-1539`). The recursive SCC's set merges successor entries with
their base key and `guard: None` (`:2270-2287`; the `anchor.is_some() &&` at `:2272`
keeps frame-bound entries when there is no anchor). **Precondition asserted** (it
proves G's entry is guarded, from the singleton path, which carries guards per edge):
N's row lacks the T2 write and M's row has it. Expected: A and B carry the T2 write.
Equal to the oracle.
**Break:** delete `anchor.is_some() && ` at `:2272` → with no anchor the recursive SCC
now skips frame-bound successor entries → the set lacks the T2 write → A's and B's
literal rows fail. (The oracle's walk reads G's cone directly, `:2051-2066`, so it still
has the write; only the new path changes.) N and M are unaffected (anchored).

### 5.5b The emission filter (T11, `scc_set_skips_members_outside_nodes`)

Tarjan follows edge targets that are not in `nodes` (`scc.rs:94-100`), so an SCC can
hold a member the cone never emits (`:2576-2578`). Graph: `nodes = [A, C]`, edges A → B,
B → A, B → C; C writes `t/C`; `direct_in` also has a fact for B (ignored: direct facts
are read only for ids in `nodes`, `:2865-2872`). **Precondition:**
`stats.recursive_members_all == 2`, `recursive_members_emitted == 1`. Expected:
`row_ids() == {A, C}` exactly, and A's row carries `t/C` (reached through B). Equal to
the oracle.
**Break:** delete the `if !routine_ids.contains(m) { continue; }` at `:2576-2578` → B
gets a row → `row_ids()` assertion fails (both paths share the filter, so only the
literal set catches it).

### 5.6 Scaling fixtures (T5, T6)

Synthetic `TypedEdge` graphs (the `graph_of` / `call_edge` helpers of
`cone_derived::tests`, `cone_derived.rs:912-944`, moved to a shared test helper):

- **T5 `scc_set_479_cycle_scales`:** a 479-member cycle, each member one distinct
  direct write, plus one successor chain of 3. Asserts: every member's row has all 482
  tables; `stats.member_walks == 0`; `stats.scc_sets_borrowed + scc_sets_built == 1`;
  `stats.scc_set_entries_folded == 479 * 482` (linear: one pass per member over one
  set); rows equal the oracle, whose `member_walks == 479`.
  **Break:** route `DerivedOnly` through the walk (call `inherited_facts_by_bfs` and fold
  its picks) → `member_walks == 0` fails.
- **T6 `scc_set_chain_and_fan_out`:** (a) chain: 20 two-member SCCs in a line, each
  with one write; (b) fan-out: one three-member SCC with 200 successor singletons.
  Asserts rows equal the oracle, `scc_sets_borrowed + scc_sets_built == 20` for (a) and
  `== 1` for (b).
  **Break:** build the set per member (call `fact_cone_for_scc` inside the member loop)
  → the set count is 40 / 3; fails.

"Guard-heavy" is T4; "producer blow-up" is T3b; the differing-states case is T3.

### 5.7 The use is pinned, derived and raw (T9, `derived_only_runs_no_member_walk`)

Over the production entry `compose_cone_over_graph`:
- **Derived:** on T5's graph under `ConeOutput::DerivedOnly`,
  `outcome.stats.member_walks == 0` and `recursive_members_emitted == 479`.
  **Break:** make `compose_cone_over_graph` pass `PerMemberWalk` → `member_walks == 0`
  fails. This is the test that fails if the production call site stops using the new
  path while every oracle stays green.
- **Raw, recursive members** (the existing mode-parity test,
  `cone_output_mode_skips_the_raw_vec_without_perturbing_the_fold`,
  `cone_derived.rs:1439`, has no recursive SCC): on T1's graph under `Both` and under
  `RawOnly`, A's raw inherited `Vec` is stated literally, order and witnesses
  included: `[ (insert, table, t/C, via "call", witness_callsite_id = A→B's call
  site), (publish, event, e/B, via "call", witness_callsite_id = A→B's call site) ]`.
  Hand trace: the walk picks `e/B` (sibling B, hop 1) before `t/C` (C's cone, hop 2,
  `:2013-2066`), and `sort_inherited` orders by `op|kind|…`
  (`:1681-1702`), so `insert` precedes `publish`. Also, for every member of T1 and T5,
  `Both`'s raw `Vec` equals `RawOnly`'s field for field, and `member_walks` is 2 (T1)
  and 479 (T5) under both modes. And `Both`'s derived rows equal DerivedOnly's.
  **Break:** in `emit_picks`, return the retagged picks without `sort_inherited`
  (`:2107`) → the walk count is unchanged, A's raw order becomes `[e/B, t/C]` → the
  literal raw assertion fails.

### 5.8 The derived-row oracle on fixtures and CDO (T7, T8)

- **T7 `scc_set_matches_member_walk_on_fixtures`:** the step-3 harness (`model`,
  `workspace_model`, `cross_app_model`, `capability_cone.rs:5765-5795`; `app_dirs`,
  `:5993-6003`; the per-root loop, `:6062-6100`): every app dir under `tests/r0-corpus`
  (217), `tests/fixtures` (29), `tests/r3a4-fixtures`, `tests/r3a5-fixtures`,
  `tests/r2-5b-fixtures` (1 each), single-app AND cross-app model. Inputs come from one
  helper extracted from `build_detector_artifacts` (`det_cone_inputs(model, rel)`:
  `nodes`, `direct_full`, `coverage_in`, the routines, `detector_context.rs:1188-1221`),
  so the oracle composes over the det cone's exact inputs. Composes twice (`PerScc`,
  `PerMemberWalk`), `assert_rows_equal`.
  **Class counts, over the fixture dirs ONLY** (the inline graphs of T1-T11 are separate
  tests and are NOT counted here, so they cannot fill a class): recursive SCCs that are
  self-loops, multi-member, roots, non-roots; recursive members with ≥ 1 own entry (the
  case §4.1 point 6 rests on); recursive SCCs with a successor whose cone holds a
  frame-bound entry (point 4). The test prints the table. The first green run's
  non-zero classes are written into the test as minimums (`>= 1`), so a fixture that
  stops reaching a class fails. A class that is 0 over the fixtures is a stated LIMIT
  of T7, recorded in the CHANGELOG entry: it is covered only by the hand-stated test
  for it (T2, T3/T3b, T4) and by T8 on CDO, which prints the same classes. r4's
  fixtures are the r0 corpus
  (`tests/r4/r4_differential.rs:1246-1247`) and its cross-app dirs
  (`r4_cross_app_program.rs:245-248`), all included.
  **Gap deviation** (as step 3): `tests/gap` keeps its scenarios as inline AL inside the
  integration crate, which cannot call a `cfg(test)` lib oracle; the fixture dirs and
  the hand-stated tests (T1-T6, T11) stand in. The gap goldens still run unchanged (I2).
  **Break:** the T1 break (skip the successor merge for a recursive SCC) → fails on
  the first fixture whose class count "multi-member or self-loop with a successor" is
  non-zero. If that class is 0 over the fixtures, the break is recorded as not
  reaching T7 (a stated limit), and T1 carries it.
- **T8 `scc_set_matches_member_walk_on_cdo`:** CDO_WS-gated like
  `relation_equals_the_legacy_base_on_cdo` (skips without `CDO_WS`, panics under
  `ENFORCE_CDO_WS=1`), cross-app and single-app. Prints
  `scc-set oracle on CDO: <n> routines, <r> recursive SCCs, <m> members, max <k>, <classes>, <s>`
  with T7's class counts; asserts `recursive_members_emitted == 714` cross-app (step
  0's BFS calls; a different number means the input moved and the spec's numbers are
  stale, so the test says so). Runs in `scripts/cdo-gate`. **Cost:** it builds the
  CDO cross-app and single-app models once and composes each twice (new path, then the
  oracle). No committed number gives its time in the `cdo` profile; step 4's CDO parity
  test took 24.6 s for its two cases (step 4 numbers file, A4.11). T8 prints its wall,
  and the numbers file records it and the cdo-gate total before/after.
  **Break:** the T3 second break (skip own entries) → CDO differs if any recursive member
  reaches a producer only through its own entries; if that breaks nothing on CDO, the
  T1 break is used and the null result is recorded as a stated LIMIT (CLAUDE.md: a green
  break is evidence about the test).

### 5.9 Task 1's instrument (T10, `compose_stats_counts_scc_shapes`)

On a hand graph with one 3-member SCC (root), one self-loop (non-root), one
non-recursive singleton: `recursive_sccs == 2`, `recursive_members_all == 4`,
`recursive_roots == 1`, `self_loops == 1`, `size_hist == [1, 0, 1, 0, 0]`. Written and
green at Task 1, before the switch.
**Break:** count a non-recursive singleton as recursive (the predicate `recursive`
becomes `recursive || members.len() == 1`) → `recursive_sccs == 3` and
`recursive_members_all == 5`; the `recursive_sccs == 2` assertion fails first.

### 5.10 Tests kept, none deleted (I2's exception unused)

`bfs_walk_picks_own_entries` (`capability_cone.rs:4659-4671`) stays: the walk stays for
the raw half. `cone_output_mode_skips_the_raw_vec_without_perturbing_the_fold`
(`cone_derived.rs:1439`) and `a_mixed_temp_keys_in_a_recursive_scc_keep_both_classes`
(`:1218`) must stay green unchanged: the second one composes a recursive SCC under
`Both` and checks the derived row against the naive union, an independent oracle for
the new path. No `--lib` test is rewritten.

## 6. The stop rule (from the Cone-SCC distribution)

Epic row 7a says "stop rule from step 0's SCC distribution". Step 0 recorded the
distribution of the PRIMARY SCC (`scc_stats`, emitted by `build_l4`,
`analysis.rs:184-224`): max 1, one self-recursive routine. Of the Cone graph it recorded
only `max_scc_members` 479 and `BFS_CALLS` 714 (amendment 7A-3). So plan Task 1 adds the
Cone-SCC census (§4.5) with no behaviour change, and takes ONE reading set before any
switch code. Every threshold below is computed from this reading, on server1, in one
session: no Windows step-0 number enters a rule (S1-4).

**The reading** (release-fast, because it reads timings; the `cdo` profile is for counts
only). It runs under the exclusive bench slot, so no build runs beside it:
`agentflow run --name bench --timeout 30 -- bash /work/bench/160-7a-t1/stoprule.sh <Task-1 sha>`.
The script runs §8's Part-2 recipe with the Task-1 binary on BOTH sides, named `t1` and
`t1b` (the same exe twice, so the harness interleaves them), `--rounds 3`, run `a`
(census OFF) and run `c` (census ON, through the wrapper). `bench_ab.py` names each run
`{cmd}-{side}-{round}` (`scripts/bench_ab.py:79`) and its trace `{name}.trace.json`
(`:40`), so this reading writes 6 traces `a-t1-{1,2,3}` and `a-t1b-{1,2,3}`, and 6
census files `c-t1-{1,2,3}.err` and `c-t1b-{1,2,3}.err`, each with a `cone_scc:` line
(both sides are the Task-1 binary). Every check below uses exactly those names and
counts; none is copied from §8 (whose sides are `parent`/`head` and whose rounds are 5):

```bash
#!/bin/bash
# /work/bench/160-7a-t1/stoprule.sh: the §6 reading, on the Task-1 commit, before any switch code
set -euo pipefail
export TREE_SITTER_AL_PATH=/work/src/tree-sitter-al
S=/work/bench/160-7a-t1; WS=$(jq -er .cdo_ws /work/alsem/.agent/config.json); mkdir -p $S/t1
T1=${1:?usage: stoprule.sh <Task-1 commit sha>}
cd /work/al-sem-issue-160-a1
# build only the expected commit, from a clean source tree (.agent/ edits are allowed)
test "$(git rev-parse HEAD)" = "$(git rev-parse "$T1^{commit}")" || { echo "FAIL worktree HEAD $(git rev-parse HEAD) is not $T1"; exit 1; }
dirty=$(git status --porcelain -- src crates Cargo.toml Cargo.lock) || { echo "FAIL git status failed in the worktree"; exit 1; }
test -z "$dirty" || { echo "FAIL worktree has uncommitted or untracked source; commit it or have a human clean it"; exit 1; }
git rev-parse HEAD > $S/t1.sha   # recorded next to the readings
# check(): the same text as §8 Part 1's heredoc
cat > $S/check.sh <<'EOF'
check() {
  local D=$1 A=$2 Bs=$3 R=$4; shift 4
  local L="$D/runs.jsonl" c s r n
  test -s "$L" || { echo "FAIL $D: no runs.jsonl"; return 1; }
  n=$(jq -s length "$L")
  test "$n" -eq $(( $# * 2 * R )) || { echo "FAIL $D: $n records, want $(( $# * 2 * R ))"; return 1; }
  for c in "$@"; do
    for s in "$A" "$Bs"; do
      for r in $(seq 1 "$R"); do
        n=$(jq -s --arg c "$c" --arg s "$s" --argjson r "$r" \
              '[.[] | select(.cmd == $c and .side == $s and .round == $r and .exit == 0)] | length' "$L")
        test "$n" -eq 1 || { echo "FAIL $D: $c/$s/round $r has $n exit-0 records, want 1"; return 1; }
      done
    done
    n=$(jq -s --arg c "$c" '[.[] | select(.cmd == $c) | .out_sha] | unique | length' "$L")
    test "$n" -eq 1 || { echo "FAIL $D/$c: $n distinct stdout SHAs across both sides, want 1"; return 1; }
  done
  echo "OK $D: $(( $# * 2 * R )) samples"
}
EOF
. $S/check.sh
cargo build --profile release-fast --bin alsem > $S/build.log 2>&1 || { echo "FAIL build"; exit 1; }
cp target/release-fast/alsem $S/t1/alsem
printf '#!/bin/sh\nALSEM_CONES_CENSUS=1 exec %s/t1/alsem "$@"\n' $S > $S/t1-cc.sh; chmod +x $S/t1-cc.sh
python3 scripts/bench_ab.py run --out $S/a --a t1=$S/t1/alsem --b t1b=$S/t1/alsem --rounds 3 --cmd a="analyze $WS --format sarif" > $S/a.log 2>&1
python3 scripts/bench_ab.py run --out $S/c --a t1=$S/t1-cc.sh --b t1b=$S/t1-cc.sh --rounds 3 --cmd c="analyze $WS --format sarif" > $S/c.log 2>&1
check $S/a t1 t1b 3 a || exit 1
check $S/c t1 t1b 3 c || exit 1
n=$( (ls $S/a/a-t1-*.trace.json $S/a/a-t1b-*.trace.json 2>/dev/null || true) | wc -l)
test "$n" -eq 6 || { echo "FAIL: $n traces in $S/a, want 6"; exit 1; }
for f in $S/a/a-t1-*.trace.json $S/a/a-t1b-*.trace.json; do
  grep -q '"context.capability_cones"' "$f" || { echo "FAIL span missing: $f"; exit 1; }
done
n=$( (ls $S/c/c-t1-*.err $S/c/c-t1b-*.err 2>/dev/null || true) | wc -l)
test "$n" -eq 6 || { echo "FAIL: $n census files in $S/c, want 6"; exit 1; }
for f in $S/c/c-t1-*.err $S/c/c-t1b-*.err; do
  test "$(grep -c 'compose: bfs_walk=' "$f" || true)" -eq 1 || { echo "FAIL compose line: $f"; exit 1; }
  test "$(grep -c 'cone_scc:' "$f" || true)" -eq 1 || { echo "FAIL cone_scc line: $f"; exit 1; }
done
python3 scripts/trace_summary.py --compare "$S/a/a-t1-*.trace.json" "$S/a/a-t1b-*.trace.json" 0 > $S/a-spans.txt
grep -q "context.capability_cones" $S/a-spans.txt || { echo "FAIL: span not in compare"; exit 1; }
grep -H "compose: bfs_walk=\|cone_scc:" $S/c/c-t1-*.err $S/c/c-t1b-*.err > $S/c-census.txt
echo "OK stoprule reading at $(cat $S/t1.sha): $S/a-spans.txt $S/c-census.txt"
```

The build runs inside the slot but before any timed run, so nothing builds beside a
timed run. It gives, on this host: `Pw`, `Bw` = median and band of `bfs_walk` over all
6 run-`c` files; `Bc` = band (max − min) of `context.capability_cones` over all 6
run-`a` traces (one binary on both sides, so the 6 are one population); `R` = median of
`root_set_probe` over the 6 run-`c` files; the `cone_scc:` counts. Every line is
recorded in the ledger. A failed or missing sample ends the reading with no numbers
(the stop rule is then not evaluated, and no switch code starts).

Rules:

- **S1, instrument check:** `members_emitted` equals the `x` count of the `compose:`
  line (both count emissions). `members_all ≥ members_emitted`. When
  `max_scc_members > 1`, `max` equals it (the all-SCC max equals the recursive max only
  then; a 1 means no multi-member SCC). A mismatch stops the step until the census is
  fixed.
- **S2, the floor is reachable:** `R`, the measured root-set construction (§4.5: the
  exact build §4.2 will run, timed directly, not projected), must be ≤ **500 ms** (half
  the 1 s floor; the other half for noise and the non-root borrows). Else stop and
  report to the conductor with the `cone_scc:` line; the design must change before code.
  `own_entries` is recorded too: the new path folds each own entry once, where the old
  folded the winning pick per key, so the fold count may differ by up to that number
  (reported under `bfs_fold`, not bounded).
- **S3, the speed bound is not void (I6, S1-6):** `Pw − 1000 − Bw ≥ 2 × Bc`. The
  cone-saving bound A7a.5 subtracts `Bc`; if the expected saving's margin is below
  twice the cone span's band, the bound can pass or fail on noise. Then I6 says the step
  is re-labelled non-speed BEFORE code: stop and return to the panel (A7a.4 and A7a.5
  would be dropped, never "passed").

No other shape can defeat the design: self-loops and multi-member SCCs both lose their
walk (a non-root self-loop's set is its already-built fact cone). The rule is about
roots, whose set 7a must build, and about noise.

## 7. Acceptance matrix

| # | Epic row 7a item | Evidence |
|---|---|---|
| A7a.1 | `DerivedOnly` reads one set per Cone-SCC | §4.2; T5 and T9 (`member_walks == 0`, one set per SCC); T6 |
| A7a.2 | Old path test-only until CDO passes | `RecursiveMembership::PerMemberWalk` is `cfg(test)` (§4.3); T8 green in `scripts/cdo-gate`; deletion point stated (§4.3) |
| A7a.3 | Goldens unchanged | the 8 golden targets (`scripts/check-goldens`) and `cargo test -p al-sem --lib` byte-identical; CDO `aldump --program-call-graph-stats` SHA-256 unchanged; CDO SARIF byte-identical at `--scope primary` and `--scope all` (§8 part 1) |
| A7a.4 | `bfs.walk` ≤ 1 s on CDO (absolute floor) | head median of `bfs_walk` over 5 census-ON `analyze` rounds ≤ 1,000 ms (§8 part 2, run `c`) |
| A7a.5 | Cone saving ≥ parent `bfs.walk` − 1 s − band | parent median − head median of `context.capability_cones` (census OFF, run `a`) ≥ P(bfs.walk) − 1,000 ms − band(bfs.walk) − band(context.capability_cones) (amendment 7A-1); both from 7a's own parent runs (Decision 20). **Void check first:** the bound counts only if `Pw − 1000 − Bw ≥ 2 × Bc` on those parent runs; a void bound BLOCKS the speed claim (A7a.4 and A7a.5 are reported "void", the step fails its speed acceptance and goes back to the panel), it never passes |
| A7a.6 | Retained ≤ 98 MB | restated (amendment 7A-2): `context.cone_derived` + `context.summaries` bytes, head ≤ parent, one `ALSEM_RETAINED_CENSUS=1` run per side (§8 part 1) |
| A7a.7 | `fold_row` / `bfs.fold` in or out | OUT (D7a-2, §4.4); `bfs.fold` reported per side, no bound |
| A7a.8 | Derived-row oracle on r0, gap, r4, CDO | T7 (r0 = r4's corpus, cross-app dirs; gap deviation stated), T8 |
| A7a.9 | Scaling fixtures: chain, fan-out, 479-SCC, guard-heavy, producer blow-up | T6 (chain, fan-out), T5 (479), T4 (guard-heavy), T3b (producer blow-up); plus T3 (members differ), T11 (emission filter) |
| A7a.10 | Stop rule from the SCC distribution | §6 (Cone-SCC census, S1-S3, same host and session), read before switch code |
| A7a.11 | M3 decided | §4.6: deferred to step 6, with reasons |
| A7a.12 | E14 CDO ratchets | `scripts/cdo-gate "$WS"` green, T8 included |

**I6 for every bound.**

| Bound | Timer | Step-0 median | Step-0 band | Median ≥ 2 × band | Re-checked |
|---|---|--:|--:|---|---|
| A7a.4 `bfs.walk` ≤ 1 s | `BFS_WALK_NANOS` (census ON) | 8,627.5 ms | 223.6 ms | yes (38.6×) | on 7a's parent runs: `Pw ≥ 2 × Bw`, else void |
| A7a.5 cone saving | `context.capability_cones` span (census OFF) against `bfs.walk` | 22,193.1 ms (parent) | 1,126.3 ms (but 7.2 s and 12.3 s at steps 1 and 1b on a loaded machine) | the margin, not the median, is what can be noise: `Pw − 1000 − Bw ≥ 2 × Bc` (step 0: 7,403.9 ≥ 2,252.6, yes) | on 7a's parent runs, and before code at §6 S3; void BLOCKS the claim |
| A7a.6 retained | census bytes (deterministic) | 103,011,037 B | 0 (exact counts) | n/a: a deterministic count, bound as head ≤ parent | equal expected |
| — `bfs.fold` (reported) | `BFS_FOLD_NANOS` | 1,029.3 ms | 31.8 ms | yes, but unbounded by D7a-2 | — |

Step 0's timer bands are head-only (the parent had no such timer then); 7a's parent has
them, so 7a reads every band from its own parent runs. Step 0 ran on Windows; server1 is
Linux, so absolute numbers differ, and every figure in the verdict comes from 7a's own
bench.

Invariants touched: I1 (no counter moves, §4.5), I2 (A7a.3), I6 (above), I7 (§4.7), I9
(§5), I10 (A7a.12). I3, I4, I5, I8, I11 untouched.

## 8. Measurement plan

One CDO bench, run ONCE after the final panel, by the conductor through
`agentflow run --name bench --timeout 45 -- <cmd>`, split in three parts so each fits
45 minutes. No cargo build runs during parts 2 and 3 (`EXCLUSIVE_GATES`,
`scripts/agentflow/cli.py:201-204`). Paths below are server1's; `B=/work/bench/160-7a`,
`WS` = the pinned CDO workspace, read from `cdo_ws` in the main checkout's `.agent/config.json`
(at `bc3ccb18`, clean: `git -C "$WS" status --porcelain` empty, checked 2026-10-20; the path
itself is kept out of git, as `agentflow sanitize` requires). `P` = the branch base `d37e8634`; `H` =
the branch head the bench runs, passed to Part 1 as its one argument
(`bash part1.sh <H>`); Part 1 refuses any other commit and records both shas in
`$B/shas.txt`. `export TREE_SITTER_AL_PATH=/work/src/tree-sitter-al`
for every build. `RAYON_NUM_THREADS` unset.

`bench_ab.py` needs `psutil` (`scripts/bench_ab.py:37`); psutil 7.1.0 imports on
server1 (conductor, 2026-10-20). `cdo-measure`'s `measure.py` is an owner-PC skill and
is not on server1 (searched 2026-10-20); part 1 substitutes SARIF byte identity, which is
stronger than a count comparison for a byte-neutral step.

**Fail closed.** `bench_ab.py` prints a non-zero child exit but still returns 0 and
still summarises the timings (`:98-114`; `summary` returns 0 too). So every part's
script starts with `set -euo pipefail`, and every check either passes or ends the part
with a non-zero exit, which `agentflow run` records as a failed gate. A part that fails
has NO readings: its bound is "not measured", which fails the step (never "met").
Part 1 writes the one check function to `$B/check.sh`; parts 2 and 3 and the §6 reading
source it (`. $B/check.sh` under `set -e` fails if the file is missing). `check D SIDE_A
SIDE_B ROUNDS CMD...` requires, for EVERY (command, side, round) of the grid, exactly
one record, with exit 0, the exact total count, and one stdout SHA per command across
both sides. Its one definition is the heredoc in Part 1 (no second copy to drift).

Per part, in addition: every run-`a` `*.trace.json` contains the span
`context.capability_cones`, and every run-`c` `.err` holds exactly one `compose:
bfs_walk=` line, plus one `cone_scc:` line on the head side. A missing line is a failed
sample and ends the part.

### Part 1: builds, findings, histogram, census, oracle line

```bash
set -euo pipefail; export TREE_SITTER_AL_PATH=/work/src/tree-sitter-al
B=/work/bench/160-7a; WS=$(jq -er .cdo_ws /work/alsem/.agent/config.json); mkdir -p $B/parent $B/head
# $B/check.sh: the check() function, written once here, sourced by parts 2-3 and §6.
# Reads bench_ab.py's records file DIR/runs.jsonl (bench_ab.py:73, one JSON object per
# line, :82); fields cmd, side, round, exit, out_sha, run (bench_ab.py:64-66).
cat > $B/check.sh <<'EOF'
check() {
  local D=$1 A=$2 Bs=$3 R=$4; shift 4
  local L="$D/runs.jsonl" c s r n
  test -s "$L" || { echo "FAIL $D: no runs.jsonl"; return 1; }
  n=$(jq -s length "$L")
  test "$n" -eq $(( $# * 2 * R )) || { echo "FAIL $D: $n records, want $(( $# * 2 * R ))"; return 1; }
  for c in "$@"; do
    for s in "$A" "$Bs"; do
      for r in $(seq 1 "$R"); do
        n=$(jq -s --arg c "$c" --arg s "$s" --argjson r "$r" \
              '[.[] | select(.cmd == $c and .side == $s and .round == $r and .exit == 0)] | length' "$L")
        test "$n" -eq 1 || { echo "FAIL $D: $c/$s/round $r has $n exit-0 records, want 1"; return 1; }
      done
    done
    n=$(jq -s --arg c "$c" '[.[] | select(.cmd == $c) | .out_sha] | unique | length' "$L")
    test "$n" -eq 1 || { echo "FAIL $D/$c: $n distinct stdout SHAs across both sides, want 1"; return 1; }
  done
  echo "OK $D: $(( $# * 2 * R )) samples"
}
EOF
. $B/check.sh
H=${1:?usage: part1.sh <head commit sha>}
HW=/work/al-sem-issue-160-a1
# head: build only the expected commit from a clean source tree (.agent/ edits are allowed)
head_ok() {
  test "$(git -C $HW rev-parse HEAD)" = "$(git -C $HW rev-parse "$H^{commit}")" || { echo "FAIL head worktree HEAD $(git -C $HW rev-parse HEAD) is not $H"; exit 1; }
  dirty=$(git -C $HW status --porcelain -- src crates Cargo.toml Cargo.lock) || { echo "FAIL git status failed in the head worktree"; exit 1; }
  test -z "$dirty" || { echo "FAIL head worktree has uncommitted or untracked source"; exit 1; }
}
# parent: an existing parent-wt is reused only if it is exactly d37e8634 and fully clean
# (untracked files included). It is never reset or cleaned here: a human removes it.
test -d $B/parent-wt || git -C /work/alsem worktree add --detach $B/parent-wt d37e8634
test "$(git -C $B/parent-wt rev-parse HEAD)" = "$(git -C /work/alsem rev-parse d37e8634)" || { echo "FAIL parent-wt is not at d37e8634; a human must remove $B/parent-wt"; exit 1; }
dirty=$(git -C $B/parent-wt status --porcelain) || { echo "FAIL git status failed in parent-wt; a human must remove $B/parent-wt"; exit 1; }
test -z "$dirty" || { echo "FAIL parent-wt is dirty or has untracked files; a human must remove $B/parent-wt"; exit 1; }
(cd $B/parent-wt && cargo build --profile release-fast --bin alsem --bin aldump) > $B/build-parent.log 2>&1
head_ok
(cd $HW && cargo build --profile release-fast --bin alsem --bin aldump) > $B/build-head.log 2>&1
printf 'parent %s\nhead %s\n' "$(git -C $B/parent-wt rev-parse HEAD)" "$(git -C $HW rev-parse HEAD)" > $B/shas.txt   # recorded next to the readings
cp $B/parent-wt/target/release-fast/{alsem,aldump} $B/parent/
cp /work/al-sem-issue-160-a1/target/release-fast/{alsem,aldump} $B/head/
for s in parent head; do
  $B/$s/alsem analyze $WS --format sarif --scope primary > $B/sarif-$s-primary.sarif
  $B/$s/alsem analyze $WS --format sarif --scope all     > $B/sarif-$s-all.sarif
  $B/$s/aldump --program-call-graph-stats $WS > $B/hist-$s.json
  ALSEM_RETAINED_CENSUS=1 $B/$s/alsem analyze $WS --format sarif > /dev/null 2> $B/census-$s.err
done
cmp $B/sarif-parent-primary.sarif $B/sarif-head-primary.sarif || { echo "FAIL SARIF differs, --scope primary"; exit 1; }
cmp $B/sarif-parent-all.sarif $B/sarif-head-all.sarif || { echo "FAIL SARIF differs, --scope all"; exit 1; }
sha256sum $B/hist-parent.json $B/hist-head.json
test "$(sha256sum < $B/hist-parent.json)" = "$(sha256sum < $B/hist-head.json)" || { echo "FAIL histogram"; exit 1; }
for b in context.cone_derived context.summaries; do
  # bucket line: "[retained-census] bucket=<name> bytes=<n> allocs=… kind=…" (src/engine/census.rs:51)
  p=$(grep -o "bucket=$b bytes=[0-9]*" $B/census-parent.err | cut -d= -f3 || true)
  h=$(grep -o "bucket=$b bytes=[0-9]*" $B/census-head.err | cut -d= -f3 || true)
  test -n "$p" || { echo "FAIL retained $b: bucket missing in census-parent.err"; exit 1; }
  test -n "$h" || { echo "FAIL retained $b: bucket missing in census-head.err"; exit 1; }
  test "$h" -le "$p" || { echo "FAIL retained $b: head=$h > parent=$p"; exit 1; }
done
head_ok   # the oracle test builds the head tree again (cdo profile)
(cd $HW && CDO_WS=$WS cargo test --profile cdo -p al-sem --lib scc_set_matches_member_walk_on_cdo -- --nocapture) > $B/cdo-oracle.log 2>&1
grep -q "scc-set oracle on CDO:" $B/cdo-oracle.log && grep -q "1 passed" $B/cdo-oracle.log || { echo "FAIL oracle line"; exit 1; }
```

Pass: both `cmp` silent; the two histogram SHA-256 equal (step 4 recorded
`c032c8db…a1594ef`); for each bucket, head `bytes=` ≤ parent `bytes=` (A7a.6; equal
expected); the oracle log has its `scc-set oracle on CDO:` line and `1 passed`.

### Part 2: bounded runs

Two harness runs, interleaved, 5 rounds each (≥ 3 per side, I6). Run `a` is census OFF
(`bench_ab.py` strips every `ALSEM_*` variable, `:39`). Run `c` needs the cone census,
which `bench_ab.py` would strip, so each side runs through a two-line wrapper written
outside the repository; `exec` keeps the PID, so the harness still measures the alsem
process:

```bash
set -euo pipefail
B=/work/bench/160-7a; WS=$(jq -er .cdo_ws /work/alsem/.agent/config.json)
cd /work/al-sem-issue-160-a1   # scripts/bench_ab.py and scripts/trace_summary.py are relative
. $B/check.sh
for s in parent head; do
  printf '#!/bin/sh\nALSEM_CONES_CENSUS=1 exec %s/%s/alsem "$@"\n' $B $s > $B/$s-cc.sh; chmod +x $B/$s-cc.sh
done
python3 scripts/bench_ab.py run --out $B/bench-a --a parent=$B/parent/alsem --b head=$B/head/alsem --rounds 5 --cmd a="analyze $WS --format sarif" > $B/bench-a.log 2>&1
python3 scripts/bench_ab.py run --out $B/bench-c --a parent=$B/parent-cc.sh --b head=$B/head-cc.sh --rounds 5 --cmd c="analyze $WS --format sarif" > $B/bench-c.log 2>&1
check $B/bench-a parent head 5 a || exit 1
check $B/bench-c parent head 5 c || exit 1
# 2 sides (parent, head) x 5 rounds: 10 traces in bench-a, 10 census files in bench-c
n=$( (ls $B/bench-a/a-parent-*.trace.json $B/bench-a/a-head-*.trace.json 2>/dev/null || true) | wc -l)
test "$n" -eq 10 || { echo "FAIL: $n traces in bench-a, want 10"; exit 1; }
for f in $B/bench-a/a-parent-*.trace.json $B/bench-a/a-head-*.trace.json; do
  grep -q '"context.capability_cones"' "$f" || { echo "FAIL span missing: $f"; exit 1; }
done
n=$( (ls $B/bench-c/c-parent-*.err $B/bench-c/c-head-*.err 2>/dev/null || true) | wc -l)
test "$n" -eq 10 || { echo "FAIL: $n census files in bench-c, want 10"; exit 1; }
for f in $B/bench-c/c-parent-*.err $B/bench-c/c-head-*.err; do
  test "$(grep -c 'compose: bfs_walk=' "$f" || true)" -eq 1 || { echo "FAIL compose line: $f"; exit 1; }
done
# the parent (d37e8634) has no cone_scc line; the head (Task 1 on) must have one per run
for f in $B/bench-c/c-head-*.err; do
  test "$(grep -c 'cone_scc:' "$f" || true)" -eq 1 || { echo "FAIL cone_scc line: $f"; exit 1; }
done
python3 scripts/trace_summary.py --compare "$B/bench-a/a-parent-*.trace.json" "$B/bench-a/a-head-*.trace.json" 0 > $B/bench-a-spans.txt
grep -q "context.capability_cones" $B/bench-a-spans.txt || { echo "FAIL: span not in compare"; exit 1; }
grep -H "compose: bfs_walk=" $B/bench-c/c-parent-*.err $B/bench-c/c-head-*.err > $B/bench-c-bfs.txt
grep -H "cone_scc:" $B/bench-c/c-head-*.err >> $B/bench-c-bfs.txt
```

(`trace_summary.py --compare` hides a span whose difference is under 200 ms unless a
threshold is given, `scripts/trace_summary.py:97`; the trailing `0` shows every span.)

Readings (written into the numbers file, every run listed):

- **A7a.5:** from `bench-a-spans.txt`, `context.capability_cones` inclusive ms: parent
  median `Pc`, band `Bc`; head median `Hc`. From `bench-c-bfs.txt`, parent `bfs_walk`
  median `Pw`, band `Bw`. Met when `Pc − Hc ≥ Pw − 1000 − Bw − Bc`. The literal row
  form (`Pc − Hc ≥ Pw − 1000 − Bw`) is computed and reported too (7A-1).
- **A7a.4:** head `bfs_walk` median ≤ 1,000 ms.
- `bfs_fold` medians and bands per side: reported (D7a-2). Expected about equal.
- stdout SHA equal on both sides in both runs (`bench_ab.py summary`), and run `c`'s
  head `cone_scc:` line shows `member_walks=0`.

A miss fails the step. The census perturbs timing, which is why the cone saving reads
run `a` and only the walk/fold timers read run `c`.

### Part 3: reported runs, no bound

```bash
set -euo pipefail
B=/work/bench/160-7a; WS=$(jq -er .cdo_ws /work/alsem/.agent/config.json)
cd /work/al-sem-issue-160-a1   # scripts/bench_ab.py is relative
. $B/check.sh
python3 scripts/bench_ab.py run --out $B/bench-r --a parent=$B/parent/alsem --b head=$B/head/alsem --rounds 5 --cmd as="analyze $WS --single-app --format sarif" --cmd px="policy check $WS --policy /work/al-sem-issue-160-a1/tests/fixtures/cli-c-policy/ws-policy-custom/al-sem.policy.yaml --format json --deterministic" > $B/bench-r.log 2>&1
check $B/bench-r parent head 5 as px || exit 1
python3 scripts/bench_ab.py summary $B/bench-r/runs.jsonl > $B/bench-r-summary.txt
# the part's exit status is check's, not summary's: summary returns 0 on any input
```

`as`: single-app has one self-loop on CDO (step 0, fingerprint `max_scc_members` 1;
BFS calls 2 over two cones), so no change is expected. `px`: cross-app policy check runs
the det cone (DerivedOnly: saves the walk) and the RAW build (Both: still walks, now
also folds the SCC set), so it shows both directions. Walls and stdout SHA reported.
Peaks: on Linux `perf_trace::read_rss` is a stub returning `None`
(`src/engine/perf_trace.rs:174-177`), so the trace prints no `peak.*` line and spans
carry no `rss_mb`; `bench_ab.py` falls back to `psutil` RSS polling (`:48`). Peaks are
reported from that column only, labelled as polled RSS.

Numbers go to `docs/measurements/2026-10-20-issue-160-step7a.md`, with the protocol
table of step 4's numbers file (machine, commits, profile, harness, commands, every run).

## 9. False-positive and failure shapes; amendments

**Failure shapes:**

- **A successor reached only through a sibling is missed:** caught by T1 and T7 (the
  successor is on a sibling's edge in T1's graph).
- **Own entries folded per SCC instead of per member** (one member's producer frame
  leaks to another): T3; the old path is the oracle on CDO (T8).
- **A root SCC's set never built:** T2.
- **Guards applied inside a recursive SCC** (a guarded successor fact wrongly dropped):
  T4.
- **The production path quietly stays on the walk** (oracles green, no saving): T9 and
  A7a.4.
- **Cost moved out of the timer** (e.g. the set built outside `BFS_WALK_NANOS`): A7a.5
  reads the whole cone span, so moved cost still counts.
- **Interning order leaks into output:** I2 goldens and CDO SARIF identity (part 1);
  §4.7 lists every id-order reader.
- **A vacuous bound:** I6 table (§7); the parent's own bands are re-checked, and the
  void check `Pw − 1000 − Bw ≥ 2 × Bc` blocks the claim when it fails.
- **A failed bench sample read as data:** §8's fail-closed checks.
- **A member Tarjan found but the cone never emits gets a row:** T11.
- **A census timer that overlaps another** (`rest=` negative): §4.5's overlap rule;
  `census_rest_is_never_negative` stays green.

**Amendments this spec asks the epic to take** (the panel decides):

- **7A-1, the cone-saving bound subtracts the cone span's band too.** Row 7a reads the
  saving off `context.capability_cones` (a different timer from `bfs.walk`) but
  subtracts only `bfs.walk`'s band. I6 subtracts the reading timer's own band whenever a
  saving is read off a timer other than the deleted sub-phase's (step 2's digest bound,
  P1-7). The cone span's band (1,126 ms parent, 2,042 ms head on step 0) is five to nine
  times `bfs.walk`'s, so without it the bound can fail on noise alone. Step-0 numbers:
  required saving 6,277.6 ms (amended) against 7,403.9 ms (literal), expected ≈ 8,600 ms.
  Both are computed; the amended one decides. Because subtracting `Bc` can leave a
  bound near zero (the cone span's band was 7.2 s and 12.3 s at steps 1 and 1b,
  `docs/measurements/2026-10-19-issue-160-step1.md:82`,
  `docs/measurements/2026-10-19-issue-160-step1b.md:104`), the bound comes with its void
  check, `Pw − 1000 − Bw ≥ 2 × Bc`, read before code (§6 S3) and on the bench's parent
  runs; a void bound blocks the speed claim.
- **7A-2, "retained ≤ 98 MB" becomes "head ≤ parent".** The 98 matches
  `context.cone_derived` (65,308,148 B) + `context.summaries` (37,702,889 B) =
  103,011,037 B = 98.24 MiB (step 0 census), which is above 98 in MiB and 103 in MB: read
  literally, today's code fails it. The epic's source (`scratchpad/cone-timing/`) is not
  in the repository. A relative bound is what I6 and Decision 22 ask for, and 7a
  changes no row, so equality is expected.
- **7A-3, the stop rule reads the Cone-SCC distribution.** Step 0's recorded SCC
  distribution is the Primary SCC's (`scc_stats`), which has no multi-member SCC on CDO;
  the stop rule needs the Cone graph's, which Task 1 adds (§6). This also explains step
  0's recorded "disagreement" (max 1 against 479): two different graphs. Its other
  oddity, `nodes` 31,658 against `sccs` 31,656 with no multi-member SCC, is two colliding
  routine ids (gap G-18): `nodes` counts `graph.nodes`, which repeats a colliding id
  (`detector_context.rs:1190` builds `nodes` from every routine; step 3's CDO oracle
  counted 31,656 distinct cross-app routines, CHANGELOG #160 step 3).
- **7A-4, M3 resolved to step 6** (§4.6).

## 10. Risks and open questions

1. **No peaks from the trace on server1.** `perf_trace::read_rss` returns `None` off
   Windows (`src/engine/perf_trace.rs:174-177`); 7a has no peak bound, so this only
   limits what part 3 reports (polled RSS).
2. **Server1 numbers differ from step 0's Windows numbers.** No rule or bound reads a
   step-0 number: §6 reads the Task-1 runs on server1, §8 reads 7a's own parent runs.
   Step-0 figures appear only as context.
3. **The fixtures may hold few recursive SCCs.** T7 counts classes over the fixture dirs
   alone; a class at 0 is a stated limit, covered by its hand-stated test and by T8.
4. **T3's and T4's hand traces** follow `cross_edge`, `across_edge` and `frame_guards`
   as cited in §5.4-§5.5. Each test asserts its precondition from the solve's or the
   singleton path's output first; if a trace is wrong, the test fails on the
   precondition (never passes vacuously), and the fixture is corrected with the reason
   recorded.
5. **The Task-1 reading costs one bench slot** (§6: 2 × 3 rounds × 2 runs of CDO
   `analyze`, about 25 minutes at step 0's walls) before any switch code.
