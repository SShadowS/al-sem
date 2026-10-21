# #160 child spec: step 7b — closed-class sets, fold memo

Status: revised after spec panel round 1 (2026-10-21; sol findings #1-#18, opus C1, I1-I7,
M1-M8). The probe was re-run for that round (ledger P15-P22).
Child of `2026-10-18-issue-160-analysis-session-design.md` (the epic): §5.6, §9 row **7b**,
E2, E10, E15, I2, I6, I7, Decisions 32, 34, 38, 43, 44, 54. Follows the shape of
`2026-10-20-issue-160-step7a-scc-membership-design.md` ("7a" below). Where this document
and the epic disagree, this document is wrong, EXCEPT the numbered amendments of §9
(7B-1 to 7B-4), each with its reason; until the epic takes them, they are open questions
for the panel.

Base: master `afd5d88f`, worktree `/work/al-sem-issue-160-a3`. It differs from 7a's bench
head `7bb9f90c` by a grammar bump only (`crates/al-syntax`, `cache_prune.rs`); no cone code
moved, and the CDO shape is the same (31,658 routines, 30,942 singleton calls, 28,191
fact cones, 714 recursive members). Every `file:line` is in that tree. Every fact the design
rests on is a row of the ledger's `## Assumption probes` table (`.agent/issue-160/ledger.md`,
P1-P22).

Words (7a's, plus):
- **F(X)** = the derived row folded from a set of facts X (`fold_fact`,
  `cone_derived.rs:736-783`, then `finish_routine`, `:785-806`). F reads a subset of the
  key's fields: `resource_kind`, `op`, `resource_id` and the known-temp class.
  `inherited_fact_key` (`capability_cone.rs:1306-1315`) has five parts (those four plus
  `confidence`), so F is a function of the key and a union (7a §4.1 points 1-2), **on one
  domain condition**: the key writes `resource_id` as `as_deref().unwrap_or("")`, so `None`
  and `Some("")` share a key while `fold_fact` treats them differently (`None` returns
  before any id is pushed). The design therefore assumes no fact has `resource_id ==
  Some("")`. Measured: 0 such facts among the closed, open and own entries the memo path
  folds on CDO (ledger P20). `fold_set` carries a `debug_assert!` on it (§4.2).
- **Closed entry** = a cone entry with no guard (`frame_bound(e)` is false,
  `capability_cone.rs:1537-1539`). **Open entry** = a frame-bound (guarded) one; it is
  carried per call edge through `substitute_entry` (`:1592-1616`) and may be dropped there.
- **Closed set of j** = the closed entries of Cone-SCC j's fact cone `fact_cones[j]`.
- **Memo row of j** = F(closed set of j), built once, at the first read.

## Feasibility first (the epic's drop rule)

Epic row 7b: X is derived before code, and the step is dropped unless X ≥ 2 × band.

**X = 4,382.1 ms.** It is a PROJECTION, not a measurement of the head: a throwaway census
patch at this base (ledger P15) ran the complete new derived path beside the real one on
CDO cross-app `analyze` (release-fast, census ON, 5 interleaved rounds against the
unpatched base, exclusive bench slot, load avg 1.5-3.9). The patch placed the new path
before the real one in each member loop, as the head will run it. One timer, `sgl_full`,
wrapped the whole singleton replacement region:
- J(m) collection and dedup;
- memo-row builds;
- `fold_row`;
- a fresh `substitute_entry` pass plus the open-entry fold;
- the own-entry fold.
`drop` timed the memo-row destruction in the refcount-free loop. `rec` timed the per-SCC set
fold plus each member's `fold_row` and own-entry fold.

| Run (round) | Base Q = `singleton + bfs_fold` | Projected head Q = `sgl_full + drop + rec` |
|---|--:|--:|
| 1 | 5,279.5 | 892.4 |
| 2 | 5,372.5 | 1,063.5 |
| 3 | 5,238.4 | 873.4 |
| 4 | 5,219.4 | 897.4 |
| 5 | 5,376.8 | 952.4 |
| **median** | **5,279.5** | **897.4** |

X = 5,279.5 − 897.4 = **4,382.1 ms**.

What the projection leaves out:
- **The head's own call overhead.** The projected side does not include the head's
  function call or the census adds the head keeps.
- **Finalisation.** The self-half fold and `finish_routine` sit in `DERIVED_NANOS` on both
  sides, outside Q, and are reported only. The probe's finalisation for singletons is
  116.8-122.5 ms.
- **Census adds inside the probe.** The probe's regions contain census `add` calls, which
  inflate the projected side a little.
- **A pre-warm the head will not have (opus N1).** For each singleton, probe 2 ran two
  extra passes just BEFORE `sgl_full` started (`probe2.patch`, the `own_keys` and `hash`
  passes), over the same data the projected region then reads:
  - the `own_keys` pass reads every own rep;
  - the `hash` pass reads every closed entry of each cone read for the first time.
  So the memo builds and the own fold inside `sgl_full` may have run partly warm. The
  evidence says the effect is small: probe 2's memo sub-timer (median 585.9 ms) is not
  below probe 1's (median 510.3 ms), and probe 1 had neither pass. It is not quantified.

The first probe round (P5) summed sub-timers instead of one region and gave 3,828.9 ms. It
is kept in the ledger as history; it no longer enters any rule.

**Bands at spec time** (they decide only this feasibility check; §8's bounds take bands
ONLY from the final bench's parent runs, and nothing from P2/P3/P15 enters a bound):

| Reading | Q band (census ON) | Cone-span band (census OFF) |
|---|--:|--:|
| P2/P3 reading 1, 5 runs | 4,241.6 (one disturbed run) | 1,502.2 (one disturbed run) |
| P3 reading 2, 6 runs (base twice, a baseline-band reading, no saving measured) | 472.1 | — |
| P15 reading 3, 5 runs | 157.4 | — |

The disturbed run of reading 1 had `bfs_fold` 1,352.4 ms (about 3.5× typical) and the
other timers 1.6-2.2× (`singleton` 7,856.1, `fact_cone` 2,940.8, `pd_solve` 5,857.2,
which 7b does not touch). The cause is not proven; server1 is shared.

**Verdict: X ≥ 2 × band holds on every reading but one.** 4,382.1 ≥ 3,004.4 (2 × the cone
span band, disturbed run included), ≥ 944.2 and ≥ 314.8 (2 × Q band, readings 2 and 3).
It fails only against reading 1's Q band with the disturbed run (8,483.2). 7b is NOT
dropped at spec time. The bench repeats the check on its own parent runs, and there a
failure is binding (§8).

## 1. Goal

A singleton's derived row is folded from **memoized rows of its callees' closed sets**,
not from one `fold_fact` per surviving key. A recursive member's SCC set is folded **once
per SCC**, not once per member. Today, under `ConeOutput::DerivedOnly`,
`inherited_facts_for_singleton` (`capability_cone.rs:1741-1958`) builds a `best` map over
every cone entry of every out-edge (12,610,531 closed entries scanned on CDO, as a
`BTreeMap<&str, Best>` keyed by string) only to fold its 10,584,764 survivors, and the
witness it keeps per key (dist, first hop) is never read in that mode. 7a's recursive
branch folds the same SCC set for each of an SCC's members (4,292,741 folds for 67,491
distinct set entries).

After 7b the derived row of every routine is
F(memo rows of the distinct callee SCCs) ∪ F(substituted open entries) ∪ F(own entries)
∪ F(self half) for a singleton, and F(SCC set row) ∪ F(own entries) ∪ F(self half) for a
recursive member. Both equal today's row exactly (§4.1). No output byte moves (I2).

## 2. Non-goals

- **No `FactKeyInterner`, no hash-consed `SetArena`** (amendment 7B-1, ledger P8, P17). The
  memo key is the callee's Cone-SCC index.
- **The raw path is unchanged.** `RawOnly` and the raw half of `Both` still build `best`
  (singletons) and walk per member (recursive): the raw facts carry a per-key witness
  (first hop, distance) that a row cannot give. `inherited_facts_by_bfs`, `emit_picks` and
  the `best` scan stay production code for those modes.
- **No change to `fact_cone_for_scc`** (1,564.8 ms, x28,191), to `pd_solve` (step 6), to
  the coverage cone, or to which routines get a row.
- **No ordering bits.** The derived row holds five presence bits (`cone_derived.rs:111-123`);
  Decision 38's ordering bits live in step 2's ordering cell (`detector_context.rs:1044`).
  7b neither reads nor adds them (P13).
- **No seeding of a callee memo row from a recursive SCC's set row.** The closed set of a
  non-root recursive SCC equals its SCC set (every guard is `None` there, 7a §4.1 point 4),
  so the row could be shared; the duplicate fold it saves is at most 67,491 entries. Not
  worth a second code path.
- **No peak bound.** The epic lists none for 7b. The memo peaks at about 4 MB (P11),
  reported.

## 3. Background: what the singleton path does today

Per non-recursive member m (`capability_cone.rs:1741-1958`):

1. `subst` (`:1771-1800`): for every out-edge, every OPEN entry of the callee cone goes
   through `substitute_entry` into m's frame (152,230 on CDO).
2. `own_keys` (`:1805`): `inherited_fact_key` of each own (producer-solve) entry — one
   `String` per entry, untimed by any sub-timer.
3. `scan_cone` (`:1814-1858`): every CLOSED entry of every cone-bearing out-edge, plus the
   substituted open entries, competes per base key in `best` (`candidate_wins`).
4. `scan_own` (`:1864-1882`): own entries compete in `best`.
5. `fold` (`:1934-1943`): `fold_fact` over `best.values()`.

Under `DerivedOnly` steps 2-5 compute a witness nobody reads: the fold is a function of the
key (F), and `best` holds exactly one entry per key of (closed ∪ substituted open ∪ own).
CDO census at base (ledger P4): `scan_cone + scan_own + fold` 4,269.3 ms median; 15,104
singletons read no cone, 7,821 one, 8,017 two or more; 51,095 cone-bearing edges, of which
8,183 repeat a (subject, callee SCC) pair already read (P12).

The recursive branch (7a §4.2) folds `fact_cones[i]` (or a root's local set) entry by entry
for each member, then the member's own entries (`capability_cone.rs` member loop, timed
`BFS_FOLD_NANOS`).

## 4. Design

### 4.1 Why the memo gives the same row

Singleton m, mode wants derived. Let E(m) = the cone-bearing out-edges of m, J(m) = their
distinct callee SCCs, O(m) = the open entries that survive `substitute_entry` over E(m),
W(m) = m's own entries.

    old(m) = F(best(m)) ∪ F(self half of m)
    new(m) = ⋃_{j ∈ J(m)} memo(j) ∪ F(O(m)) ∪ F(W(m)) ∪ F(self half of m)

1. keys(best(m)) = ⋃_{edges} base keys of the closed entries ∪ base keys of O(m) ∪
   keys(W(m)): every candidate inserts its key the first time, later ones only replace the
   winner (`:1840-1857`, `:1866-1880`). A closed entry's base key is its key
   (`base_key_of`, `:1531-1533`).
2. F is a function of the key and a union (7a §4.1 points 1-2, under the `Some("")`
   domain condition in Words), so
   F(best(m)) = F(closed entries over E(m)) ∪ F(O(m)) ∪ F(W(m)). The condition matters here
   more than in 7a: `best` folds ONE rep per key, while the memo folds every closed entry,
   so two reps under one key that F tells apart (`None` vs `Some("")`) would give
   different rows.
3. The closed entries over E(m) depend only on the callee SCC, not the edge: the scan reads
   `cones[scc(edge.to)]` filtered by `!frame_bound` (`:1834-1837`) with no edge-dependent
   step. So their union over E(m) is ⋃_{j ∈ J(m)} closed set of j, and F of it is
   ⋃ memo(j).
4. memo(j) is valid for every reader: `fact_cones[j]` is inserted once (`:2597`) and never
   mutated before its refcount-free; the memo row is dropped at the same free (§4.4).

Recursive member m of SCC i: 7a's new(m) = F(SCC set of i) ∪ F(own entries of m) ∪
F(self half). F(SCC set of i) does not depend on m, so it is folded once per SCC and
`fold_row`-ed per member.

Probe evidence: the memo construction, run beside the real path on CDO cross-app,
reproduced **31,656 of 31,656** rows (singletons and recursive members) in each of its
five runs, in both probe rounds:
- Round 1 (ledger P7) compared flags and the four resolved id lists only.
- Round 2 (P16) used 7a's complete comparator: row-id sets, flags, the four lists AND the
  `physical_table_write_ops_of` masks.

### 4.2 The builder: one classifier, two entry points (`cone_derived.rs`)

`fold_fact`'s body moves into a private `Scratch::add(&mut self, interner, f)` over a
`Scratch { flags, writes_all, phys_writes, phys_reads, events }` struct, so there is ONE
definition of what a fact contributes. `ConeDerivedBuilder` holds two scratches: `cur`
(the routine in progress, today's four `s_*` buffers and `flags`) and `set` (for memo
rows, so a row can be built while a routine is in progress — the singleton builds its
callees' rows after `begin_routine`).

```rust
/// F of a closed set, frozen (sorted, deduped, masks OR-merged). Its ids are this
/// builder's `ResId`s; meaningless with any other builder.
pub(crate) struct FoldedRow {
    flags: u8,
    writes_all: Box<[ResId]>,
    phys_writes: Box<[(ResId, u8)]>,
    phys_reads: Box<[ResId]>,
    events: Box<[ResId]>,
}

impl ConeDerivedBuilder {
    pub fn fold_fact(&mut self, f: &CapabilityFact);          // unchanged signature
    pub(crate) fn fold_set<'f>(&mut self, facts: impl IntoIterator<Item = &'f CapabilityFact>) -> FoldedRow;
    pub(crate) fn fold_row(&mut self, row: &FoldedRow);        // OR flags, extend `cur`
}
```

`fold_set` carries `debug_assert!(f.resource_id.as_deref() != Some(""))` for each fact
(the Words domain condition; 0 such facts on CDO, P20). A debug build of any fixture that
breaks it fails loudly instead of folding a row that differs from the old path.

No existing fixture or test is expected to trip it (P23):
- A fact's `resource_id` is set in production from two places only: `op.table_id`
  (`capability_cone.rs:609`) and `evt.id` (`:1092`). Every other construction passes
  `None`.
- `op.table_id` is always a model table id (`src/program/model/record_types.rs:138-248`,
  built from `{app_guid}/table/{n}`, `workspace.rs:592`).
- `evt.id` is an encoded event id.
- No hand-built test fact in `src/` or `tests/` passes `Some("")` as a resource id. The
  `Some("")` hits in a grep are other fields: routine-id parts, record-variable
  names, a regen flag.
The first `cargo test` run with the assertion is the check. If it trips, the domain
condition is false for that input: §8's T8 mismatch rule applies (name the class or drop).

`finish_routine` is unchanged: it already sorts, dedups and OR-merges the masks of
`cur` (`freeze_ids`, `freeze_masked`, `:842-870`), so ids that arrive twice (from two rows,
or a row and a fact) collapse as they do today. `fold_set` interns through the same
`store.interner`, so memo rows and per-fact folds share ids. The set of interned resource
ids is unchanged (every closed entry's rid was a key winner's rid before), so the retained
`context.cone_derived` bytes are equal (§4.6).

### 4.3 The singleton path

`inherited_facts_for_singleton` gains `memo: &mut ClosedRowMemo` and the stats. With
`mode.wants_derived()`, after `subst` (unchanged, it feeds both halves):

1. J(m): the distinct callee SCCs of the cone-bearing out-edges, in out-edge order (a
   `Vec<usize>`, `contains` before push). Measured on CDO (P19): the largest singleton
   out-degree is 292 and the largest J(m) is 156. Each edge's `contains` scans at most
   the 156 entries collected so far, so the worst subject does at most
   292 × 156 = 45,552 comparisons; that is a bound, not a count. The probe's whole region,
   J collection included, is in X. A set-backed dedup is not needed.
2. For each j in J(m): `memo.entry(j).or_insert_with(|| derived.fold_set(closed entries
   of cones[j]))`, then `derived.fold_row(&memo[j])`.
3. `derived.fold_fact(&e.rep)` for each substituted open entry, and for each own entry.

With `!mode.wants_raw()` it returns here: no `own_keys`, no `best`, no scan. With
`mode.wants_raw()` (RawOnly never wants derived; Both) the `best` scan runs as today and
builds the raw `Vec` only; its fold of `best.values()` (`:1934-1943`) is deleted. So in
production the derived row has ONE source in every mode, as 7a did for recursive members.

`ClosedRowMemo = HashMap<usize, FoldedRow>` is a local of `compose_inherited_cones`, next
to `fact_cones` (`capability_cone.rs:2545`).

### 4.4 Lifetimes (Decision 32: build-local)

- A memo row of j is built at its first read and removed in the refcount-free loop at the
  same point as `fact_cones[j]` (`fact_cones.remove(y)` in the loop after the member loop).
  It never outlives its cone. CDO: at most 1,568 rows / 836,418 ids live (≈ 4 MB, P11).
- The leak `debug_assert!` after the SCC walk (`fact_cones.is_empty() &&
  cov_cones.is_empty()`) gains `&& memo.is_empty()`, with its message naming the memo.
  Memo destruction is measured: 2.5-2.7 ms per run (P15 `drop`), included in X.
- A recursive SCC's set row is a local of the member loop, dropped after it.
- Nothing new reaches `CapabilityAnalysis`, `ConeDerivedStore` or any cell. `FoldedRow`
  is `pub(crate)` and never stored in the store.

### 4.5 The recursive branch

For each recursive SCC with `use_set` (7a §4.2): `let set_row = derived.fold_set(set.values()
.map(|e| &*e.rep))` once, then per member `fold_row(&set_row)` and `fold_fact` per own
entry. The per-member `for entry in set.values()` loop is deleted.

**Timer window (M4).** The `fold_set` call sits AFTER the `add_since(BFS_WALK_NANOS,
_t_set)` that closes the set build/borrow window (`capability_cone.rs:2686-2703`). It runs
under its own `Instant`, added to `BFS_FOLD_NANOS` only. If it sat inside `_t_set` it
would be timed twice (walk and fold), and 7a's `bfs_walk` reading would move. The overlap
rule of 7a §4.5 holds.

**`scc_set_entries_folded` keeps its member-weighted meaning** (opus I3): Σ set length
over the emitted members that read a set. Its doc is reworded from "folded" to "entries
the members' rows cover (each member reads the SCC's set row once)". Its census name
`set_entries=` is unchanged. The three tests that pin it stay unchanged and green:
- 7a T3b `== 8` (`capability_cone.rs:7332`);
- 7a T5 `479 * 482` (`:7378`);
- `census_cone_scc_line_counts_the_recursive_sccs` `set_entries=18`
  (`tests/cli/cli_build_counters.rs:785`).
The once-per-SCC work is a new counter, `scc_set_folds` (§4.8).

### 4.6 Order and determinism (I7)

As 7a §4.7: rows are equal by content; the INTERNING ORDER of resource ids changes (memo
rows are folded in `BTreeMap` key order at first read, not in `best` order). Interning
order never reaches output (every accessor resolves and sorts by string,
`cone_derived.rs:499-646`; the readers 7a §4.7 lists). The retained census counts pool
lengths and interned bytes, both order-independent.

### 4.7 The old path, test-only, then deleted

`RecursiveMembership::PerMemberWalk` (`capability_cone.rs:2400-2411`, `cfg(test)`) is
widened to mean "the pre-7b derived paths". Under it:
- **a singleton builds NO memo row and calls no `fold_row`.** Its derived row is today's
  fold of `best.values()` and nothing else; the scan runs even in `DerivedOnly`;
- a recursive member folds its walk's picks (7a).
So the old side cannot share a memo defect with the new side (opus I1). Its doc says so;
the name stays, since it lives one more commit. T7/T8 (7a §5.8) then compare every row,
singletons included, new against old. They also assert that each side ran its own path
(§5, T7/T8).

**The run that licenses the deletion (opus I5).** On the commit just BEFORE the deletion
commit, the implementer runs
`python scripts/agentflow run --name cdo-gate --timeout 45 --cwd <worktree> -- bash scripts/cdo-gate`.
The ledger's `## Gate results` records:
- that commit's SHA;
- the exit status;
- both `scc-set oracle on CDO (…)` lines, with their `closed_memo_builds` and
  `singleton_scans` figures.
The deletion commit is made only if that record exists and is green. Without it, the
variant stays and the step is not finished.

**Deleted in 7b's last commit**, after that record: the variant, its arms in
`compose_inherited_cones` and in the singleton path, and the `membership` parameter
wherever only the variant used it.

**The final oracle (T7'/T8') must survive step 6.** Step 6 deletes `ConeOutput::Both`
(epic §9 row 6), and the test must fit the 16 GiB container `cdo-gate` runs in. So each
universe composes TWICE over the det cone's exact inputs (`det_cone_inputs`):

1. `DerivedOnly` with the production path. This gives the store under test.
   - Every composition asserts the explicit zero-scan checks: `member_walks == 0` and
     `singleton_scans == 0`.
   - `closed_memo_builds > 0` is NOT asserted per composition. A fixture can legitimately
     build no memo row: `tests/r0-corpus/ws-d7-event-cycle` holds only recursive
     participants (sol S1-7).
   - T7' sums `closed_memo_builds` over its whole corpus and asserts the sum `> 0`.
   - T8' asserts `> 0` on the CDO cross-app universe, where the probe counted 25,498
     builds (P5). Single-app is not asserted, since its count was not measured.
2. `RawOnly`, the witness engine step 6 keeps. It runs through a `cfg(test)` streaming
   entry, `compose_raw_streaming(…, sink: &mut dyn FnMut(&str, Vec<CapabilityFact>))`.
   This passes a `raw_sink: Option<&mut dyn FnMut(…)>` to `compose_inherited_cones`;
   production callers pass `None`. At each member's emission the raw `Vec` goes to the sink
   instead of into `out` (its `InheritedConeResult` keeps an empty `Vec`). The sink folds
   `raw ∪ direct_raw[m]` into an oracle builder at once: `begin_routine`, `fold_fact` each
   fact, `finish_routine(m)`, the steps of `fold_routine` (`cone_derived.rs`, `cfg(test)`),
   which the sink calls directly. Then it drops the `Vec`.

   The self half is `direct_raw`, the raw set with producers, not the deduped `direct`.
3. `rows_diff(store, oracle_store)` (7a's comparator: row ids, flags, the four lists, the
   op masks).

The equality holds by §4.1 and 7a §4.1: the raw half is `best` (singletons) and the
per-member walk (recursive), and both are independent of `fold_set`, `fold_row` and the
memo. The two sides share only the classifier (`Scratch::add`), which the naive-union tests
pin (`assert_physical_writes_equal_naive_union`,
`a_mixed_temp_keys_in_a_recursive_scc_keep_both_classes`).

**Memory plan.** Resident at once:
- the universe's model;
- the `DerivedOnly` store (`context.cone_derived`, 65.3 MB on CDO, 7a's census);
- an oracle store of the same size;
- the `RawOnly` composition's working set (the same cones and `pd_solve` state as a
  `DerivedOnly` compose);
- ONE subject's raw `Vec`.
No raw `Vec` of all routines is ever held (the 14.8 M-fact / 10 GB shape step 0 measured
for the full graph). Cross-app and single-app run one after the other, each model dropped
before the next, as T8 does today. The peak should be T8's (two `DerivedOnly` composes)
plus about 65 MB.

The commit that introduces T8' records its max RSS in the ledger (run alone,
`/usr/bin/time -v`, `cdo` profile), next to T8's measured at the commit before. The rule is
T8' ≤ T8 + 256 MB.
- If T8' exceeds that, the streaming is wrong: fix it.
- If T8 itself does not fit the cap, that is a pre-existing problem, recorded as a
  discovery.

**Every user of `PerMemberWalk` and how it migrates** in the deletion commit (sol #8,
opus M5):

| Helper / test | Today | After deletion |
|---|---|---|
| `compose(…, how)` (`capability_cone.rs:6660`) | takes `RecursiveMembership` | loses `how`; takes `ConeOutput` only |
| its direct caller 7a T10 `compose_stats_counts_scc_shapes` (`:6984`, call at `:6994-7000`, passes `RecursiveMembership::PerScc`) | one `DerivedOnly` compose, `ComposeStats` assertions | drops the `PerScc` argument; its assertions are unchanged (sol S1-8) |
| `compose_model(…, how)` (`:6751`), used only by `both_paths_model` | takes `RecursiveMembership` | loses `how`; takes `ConeOutput` only (sol S1-8) |
| `both_paths` (`:6671`): (`PerScc`, `PerMemberWalk`), both `DerivedOnly` | new vs old | renamed `derived_and_raw_fold`: returns the `DerivedOnly` outcome, the `RawOnly` outcome, and the raw-fold oracle store (`raw ∪ direct` per routine, via `fold_routine`) |
| its callers: 7a T1 `scc_set_rows_hand_stated` (`:6726`), T11 `scc_set_skips_members_outside_nodes` (`:6958`), T2 `scc_set_root_and_self_loop` (`:7287`), T5 `scc_set_479_cycle_scales` (`:7359`), T6 `scc_set_chain_and_fan_out` (`:7407`, `:7429`), `scc_set_none_for_an_scc_with_no_emitted_member` (`:7499`) | hand-stated rows + `assert_rows_equal(new, old)` | hand-stated rows unchanged; `assert_rows_equal(new, raw_fold)`. T5's `old.stats.member_walks == 479` becomes `raw.stats.member_walks == 479` (RawOnly walks every member), so the assertion is kept, not dropped |
| `both_paths_model` (`:6769`) | same over a model | same change over `det_cone_inputs(m)` |
| its callers: 7a T3 `scc_set_keeps_own_entries_per_member` (`:6852`), T4 `scc_set_carries_guarded_successor_entries` (`:6920`), T3b `scc_set_own_entries_scale` (`:7318`) | precondition asserts on `new.pd_solution`, rows, new vs old | unchanged preconditions and rows; oracle = raw fold |
| `check_model` (`:7066`), used by T7/T8 | new vs old | becomes T7'/T8' above (`DerivedOnly` vs streamed raw fold) |
| `singleton()` (`:4805`), used by `singleton_walk_emits_own_entries_retagged` (`:4827`) and `singleton_walk_folds_own_entries` (`:4842`) | calls `inherited_facts_for_singleton` directly | edited in 7b's FIRST code commit to pass the new `memo` and stats arguments (and `membership` until the deletion). The two tests' assertions are unchanged; the second now exercises the memo path's own-entry fold |

No test is turned into a comparison of the new path against itself. I2's exception covers
exactly the rows above, all in the deletion commit except `singleton()`, which only gains
arguments.

### 4.8 Timers and counters

`SINGLETON_NANOS`, `BFS_FOLD_NANOS`, `SGL_SUBST_NANOS` and `SGL_RAW_NANOS` keep their names
and regions. Inside the singleton:
- `SGL_SCAN_CONE_NANOS`, `SGL_SCAN_OWN_NANOS`: the raw-mode scan only (0 in `analyze`);
- `SGL_FOLD_NANOS`: deleted with the fold of `best`;
- new `SGL_MEMO_NANOS` (memo-row builds), `SGL_ROW_NANOS` (`fold_row`), `SGL_OPEN_NANOS`
  (fold of substituted open entries), `SGL_OWN_FOLD_NANOS` (own-entry fold).
None is a component of `COMPOSE_REST_NANOS` (`component_snapshot`, eight components,
unchanged); they nest inside `SINGLETON_NANOS` exactly as today's `SGL_*` do, so
`census_rest_is_never_negative` stays green.

`ComposeStats` gains five counters:
- `closed_memo_builds`;
- `closed_memo_reads`: rows folded into a subject, i.e. `fold_row` calls on memo rows;
- `closed_memo_entries`: closed entries folded into memo rows;
- `scc_set_folds`: one per recursive SCC with a set;
- `singleton_scans`: calls that built `best`.
The census also keeps `memo_live_max`, a gated `max` atomic. It is not in `ComposeStats`;
no test asserts it.

One new census line after `singleton pop:`:
`[cones-census]   closed: memo_builds=… memo_reads=… memo_entries=… scc_set_folds=… singleton_scans=… memo_live_max=…`.
Expected on CDO cross-app `analyze`: 25,498 / 42,912 / 3,879,321 / 67 / 0 / 1,568 (P5,
P11, P12, unchanged in P15). The `singleton:` line drops `fold=` and gains `memo= row= open= own=`; no test
pins that line (P14). The build counters (`build.cone`, …) do not move (I1).

## 5. Test plan

Every new test lists its break, the failing assertion and the revert (I9; CLAUDE.md
discrimination proofs, breaks made with Edit and asserted to apply).

- **T1 `closed_memo_folds_a_shared_callee_once`** (the epic's "shared-callee-set fixture").
  Hand graph: singletons A → C, B → C, C → D; D writes `t/D`, C writes `t/C` (C and D
  non-root). Rows hand-stated (A and B: `t/C`, `t/D`). `stats.closed_memo_builds == 2`
  (C's and D's cones: D's for C's own row, C's once for A and B), `closed_memo_reads == 3`.
  **Break:** never insert into the memo (rebuild every read) → `closed_memo_builds == 3`
  (C's cone folded twice, "disable the memo → two"); the assertion fails, rows stay equal.
- **T2 `closed_memo_excludes_frame_bound_entries`**: callee C writes `t/G` under a
  boolean-parameter guard; A calls C with the contradicting literal, B with the enabling
  one (the guard fixtures 7a's T4 builds on). A's row lacks `t/G`, B's has it. **Break:**
  `fold_set` over every entry, not only closed ones → A's row gains `t/G`.
- **T3 `duplicate_edges_fold_one_row`**: A calls C at two call sites.
  `closed_memo_reads == 1` for A's composition. **Break:** drop the J(m) dedup → 2.
- **T4 `scc_set_folds_once_per_scc`** on 7a T3's source (`T3_SRC`, `capability_cone.rs`
  "-- T3, T3b --"). P and Q form one recursive SCC with successor W. Q holds a state of W's
  producer under `[Q.Flag = true]`, and P's `Q(false)` contradicts it, so the resolved own
  entries are ASYMMETRIC. (7a's T3b is an unguarded cycle whose producer states reach
  every member, so it cannot show member-local ownership; sol #9.)
  Preconditions, asserted first as in 7a T3:
  - `non_seed_states(pd, Q) >= 1`;
  - `non_seed_states(pd, P) == 0`.
  Then:
  - `scc_set_folds == 1`;
  - Q's row writes `Cust`;
  - P's row writes nothing.
  **Breaks:**
  - fold the set per member → `scc_set_folds == 2`, so the count assertion fails;
  - fold every member's own entries into the SCC set row → P's row gains `Cust`, so the
    P-row assertion fails.
- **T5 `derived_only_builds_no_best`** (pins the USE): `DerivedOnly` → `singleton_scans ==
  0`; `Both` → `singleton_scans ==` the singleton count, and its raw `Vec`s equal the parent's
  (a hand graph whose raw output is stated). **Break:** route `DerivedOnly` through the
  scan → non-zero.
- **T6 memo freed**: the extended leak `debug_assert!` (§4.4) runs in every test that
  composes, since `cargo test` builds with debug assertions on.
  **Break:** skip `memo.remove(y)`. T1 then panics on that `debug_assert!` ("cone leak: …
  memo …"), before any `ComposeStats` assertion runs. The failing assertion is the
  `debug_assert!`, named in the proof. No `ComposeStats` live-count field is added.
- **T7/T8 (7a's oracle), widened** by §4.7: every row new vs old on the fixture dirs and on
  CDO, single-app and cross-app. Each side is checked to have run its own path:
  - the `PerScc` side: `singleton_scans == 0` per composition (a quiet fallback to the
    old path fails); `closed_memo_builds > 0` summed over T7's whole corpus and on T8's
    CDO cross-app universe, not per composition (see §4.7, sol S1-7);
  - the `PerMemberWalk` side: `closed_memo_builds == 0` and `closed_memo_reads == 0` (the
    oracle shares no memo, opus I1).
  **Breaks:**
  - skip the own-entry fold in the singleton path → CDO differs (1,681,935 own entries,
    P5);
  - T2's break (memo rows over every entry, frame-bound ones included), re-run against T8
    alone (`cargo test --profile cdo -p al-sem --lib scc_set_matches_member_walk_on_cdo`).
    If CDO has no contradicted guard that reaches a memo row, this second break stays
    green. That is recorded as a stated LIMIT, and T2 carries the class.
  In the last commit they become T7'/T8' (§4.7). The same two breaks are re-run against
  them, plus a third: let the `DerivedOnly` side take the scan → its `singleton_scans == 0`
  assertion fails.
- **T9 census line** (`tests/cli/cli_build_counters.rs`):
  `census_closed_line_counts_the_memo` on a small fixture: the line exists once and
  `memo_builds > 0`. **Break:** never count builds → 0.

Kept unchanged and green: `assert_physical_writes_equal_naive_union` and
`a_mixed_temp_keys_in_a_recursive_scc_keep_both_classes` (independent naive-union oracles),
`cone_output_mode_skips_the_raw_vec_without_perturbing_the_fold`,
`derived_only_runs_no_member_walk`, 7a's T10, `census_rest_is_never_negative`,
`census_cone_scc_line_counts_the_recursive_sccs` (its `set_entries=18` included, §4.5).
7a's T1-T6 and T11 keep their hand-stated assertions. In the deletion commit their oracle
comparison moves from new-vs-old to new-vs-raw-fold, through the migrated helpers in
§4.7's table.
I2's exception covers exactly that table: T7/T8 rewritten as T7'/T8', the helpers
`compose`/`compose_model`/`both_paths`/`both_paths_model`/`check_model` migrated (T10
drops one argument), and their callers'
oracle line changed. `singleton()` only gains arguments. No other `--lib` test is deleted
or rewritten.

## 6. Acceptance matrix

| # | Epic row 7b item | Evidence |
|---|---|---|
| A7b.1 | `FactKeyInterner`, hash-consed `SetArena` | **not deliverable as named, because** the measured gain is small: duplicate memo builds cost 304.1-403.3 ms (median 314.4) gross, and hashing the sets to find them costs 182.1-198.6 ms (median 183.4), so the net is about 131 ms, below 2 × every Q band measured (P17). A key interner would cut the hashing cost, at a cost of its own. Amendment 7B-1 replaces both by a memo keyed on the callee Cone-SCC |
| A7b.2 | closed/open split | §4.3: closed → memo rows, open → `substitute_entry` + `fold_fact`; T2 |
| A7b.3 | per-`SetId` memo (SetId = callee Cone-SCC, 7B-1) | §4.3-4.4; T1 (one fold per shared callee; memo off → two), T3, T6 |
| A7b.4 | `fold_row` | §4.2; T1, T4; recursive sets folded once (T4) |
| A7b.5 | goldens unchanged | the 8 golden targets (`scripts/check-goldens`) and `cargo test -p al-sem --lib` outcomes unchanged; CDO `aldump --program-call-graph-stats` SHA-256 unchanged; CDO SARIF byte-identical at `--scope primary` and `--scope all` (§8 part 1) |
| A7b.6 | 7a's oracle | T7/T8 widened (§4.7), green in the recorded `cdo-gate` run at the commit before the deletion; then T7'/T8' (`DerivedOnly` vs streamed `RawOnly` fold, survives step 6) |
| A7b.7 | X re-derived and written before code; X ≥ 2 × band or drop | "Feasibility first" above and ledger P15: X = 4,382.1 ms (a projection of the complete new region); met at spec time, re-checked bindingly at the bench (§8) |
| A7b.8 | (scan + fold + set-building) saving ≥ X − band | B1 (§8): Pq − Hq ≥ 4,382.1 − Bq on Q = `singleton + bfs_fold` (amendment 7B-2) |
| A7b.9 | cone saving ≥ X − band | B2 (§8): Pc − Hc ≥ 4,382.1 − Bq − Bc (amendment 7B-3); the literal epic form Pc − Hc ≥ 4,382.1 − Bc is reported beside it |
| A7b.10 | `PerMemberWalk` deleted | 7b's last commit, only after the recorded `cdo-gate` run (§4.7) |
| A7b.11 | E14 CDO ratchets | `scripts/cdo-gate` green |
| A7b.12 | retained bytes | `context.cone_derived` + `context.summaries`, head ≤ parent (expected equal, §4.2) |

Invariants touched: I1 (no build counter moves), I2 (A7b.5), I6 (§8), I7 (§4.6), I9 (§5),
I10 (A7b.11). I3, I4, I5, I8, I11 untouched.

## 7. False-positive and failure shapes

- **A guarded fact folded unconditionally** (open entry in a memo row): T2.
- **Own entries dropped or shared across subjects** (the memo is per callee, own entries are
  per subject): T4, T8 (break).
- **A duplicate edge counted twice**: harmless for the row (union), but T3 pins the dedup so
  the cost claim holds.
- **A memo row read after its cone is freed** or kept past it: §4.4 frees them together;
  the leak assertion and T6.
- **The production path quietly stays on the scan** (oracle green, no saving): T5, T8's
  `singleton_scans == 0`, and B1.
- **Cost moved out of the bounded timer** (e.g. memo builds outside `SINGLETON_NANOS`):
  memo builds are inside it by construction (§4.3); B2 reads the whole cone span anyway.
- **Interning order leaks into output**: I2 goldens and CDO SARIF identity (§8 part 1).
- **A noise-only pass or fail**: the void checks in §8. A void bound never passes.
- **A `resource_id` of `Some("")`** (a key that hides two reps F tells apart): the
  `debug_assert!` in `fold_set` (§4.2); 0 on CDO (P20).
- **The oracle runs out of memory in `cdo-gate`**: T8' streams per subject (§4.7, memory
  plan, with a recorded RSS rule).
- **The `Both` mode pays twice** (scan for raw plus memo for derived): reported in §8
  part 3 (cross-app `policy check` wall), as 7a did. Expected neutral or better: the
  deleted fold of `best` (1,105-2,484 ms per base run) outweighs the memo builds
  (503.5-701.8 ms per probe run).

## 8. Measurement plan

One CDO bench, run ONCE after the final panel, by the conductor through
`agentflow run --name bench --timeout 45 -- <cmd>`, in 7a's three-part shape (7a §8; its
`check.sh` fail-closed rules are reused verbatim). `B=/work/bench/160-7b-final`, `WS` = the
pinned CDO workspace read from `cdo_ws` in the main checkout's `.agent/config.json`, `P` =
the branch base `afd5d88f`, `H` = the branch head. `export TREE_SITTER_AL_PATH` to the main
checkout's grammar clone for every build; `RAYON_NUM_THREADS` unset; release-fast.

- **Part 1**: build both sides from clean trees; SARIF `cmp` at `--scope primary` and
  `--scope all`; histogram SHA equal; retained buckets head ≤ parent; T8 (or T8' on the
  last commit) on CDO with its oracle lines.
- **Part 2**: `analyze $WS --format sarif`, 5 interleaved rounds per side. Run `a`
  (census OFF): span `context.capability_cones` per run. Run `c` (census ON): per run
  Q = `singleton=` + `bfs_fold=` from the `compose:` line, plus the `singleton:`,
  `closed:` and `compose rest:` lines (reported).
- **Part 3** (reported, no bound): `analyze --single-app` and cross-app `policy check`,
  5 rounds per side, walls and polled RSS.

**Bounds.** Bands come ONLY from this bench's own parent runs: band = max − min of the 5
parent runs of that reading (I6, Decision 20). Nothing from P2, P3 or P15 enters a bound.
X is fixed above, before code.

| Bound | Reading | Rule | Void check (read first) |
|---|---|---|---|
| B1 (A7b.8) | Q = `singleton + bfs_fold`, census ON | `Pq − Hq ≥ 4,382.1 − Bq` | `4,382.1 ≥ 2 × Bq` |
| B2 (A7b.9) | span `context.capability_cones`, census OFF | `Pc − Hc ≥ 4,382.1 − Bq − Bc` | `4,382.1 − Bq ≥ 2 × Bc` (7a's margin rule) |

Reported beside B2, deciding nothing: the literal epic form `Pc − Hc ≥ 4,382.1 − Bc`, as
7a did (7a measurement doc, Part 2).

Expected (P15): Hq ≈ 0.9-1.1 s against Pq ≈ 5.3 s, a saving of ≈ 4.2-4.4 s. The span
should fall by about the same.

**A void bound (pre-stated, binding).** A void bound never passes.
- If either check is void, **re-run Part 2 once** (fresh 5 rounds per side, same exclusive
  slot), and read both checks and both bounds from the re-run alone.
- If a check is void again, the speed acceptance (A7b.8, A7b.9) **FAILS**. That is the drop
  path below. It is never "merge byte-neutral with void recorded".

**A bound met-not-void but MISSED (pre-stated).** The step is dropped:
- revert the step's speed commits;
- keep no memo;
- `PerMemberWalk` is deleted by the next step that edits `compose_inherited_cones` (step 6
  at the latest, 7a §4.3);
- record in the ledger which bound missed, with the head's sub-timers (`memo`, `row`,
  `open`, `own`, `bfs_fold`) as the explanation.
There is no second design attempt within 7b.

**A row mismatch in T8 or T8' on CDO (pre-stated).** One census or probe run must name the
class of the differing rows. There are three outcomes, and every one ends in a decision:
1. **The class is closed entries whose fold depends on more than their key** (for
   example `Some("")` resource ids; P7, P16 and P20 say none exists). Route that class
   through the per-fact path (`fold_fact`, as open entries go) and state it in §4.1. Then
   re-run, in this order:
   - T8 on CDO, which must be green;
   - one new probe run, to recompute X with the class moved;
   - the §8 bench with that X.
   Drop 7b if T8 is not green, if the new X < 2 × band of that run's base side, or if the
   bench fails a bound.
2. **The class is anything else** (for example wrong mask merging in `fold_row` or
   `finish_routine`, a wrong memo lifetime, or a J(m) error). That is a defect in the
   design's own code. If one fix, verified by T8 on CDO being green and the §8 bound
   being met, does not close it, **drop 7b**. No second design attempt.
3. **One probe run cannot name the class: drop 7b.**

## 9. Amendments this spec asks the epic to take

- **7B-1, no `FactKeyInterner` / `SetArena`: the memo key is the callee Cone-SCC.** The
  epic named hash-consed sets so that equal sets share one fold. On CDO the closed sets
  that are read come from 25,498 cones, of which 14,438 are distinct.
  Measured, entry-weighted (P17):
  - the 11,060 duplicate builds fold 2,047,522 of the 3,879,321 memo entries (52.8 %);
  - those builds cost 304.1-403.3 ms (median 314.4);
  - hashing each new closed set to find the duplicates (`DefaultHasher` over the key
    strings) costs 182.1-198.6 ms (median 183.4).
  Hash-consing would therefore save about 131 ms net, below 2 × any Q band measured (314.8
  at the quietest reading). It would need a key interner to hash more cheaply, and that
  interner has a cost of its own. Per-singleton union sets (7,825 distinct of 15,838) would
  save part of `fold_row`, which costs 5.6-6.0 ms in total. A cone is already an
  immutable, uniquely numbered set while it lives (§4.1 point 4), so its index is a valid
  SetId.
- **7B-2, the bounded timer is Q = `singleton` + `bfs_fold`.** The row says "(scan + fold
  + set-building)", but at head those are differently named timers (scan and fold are
  gone; memo and row are new). I6 asks for the SAME timer on both sides; `SINGLETON_NANOS`
  and `BFS_FOLD_NANOS` wrap all of that work on both sides (including `subst`, unchanged).
  Sub-timers are reported.
- **7B-3, the cone bound subtracts both bands** (7a's 7A-1 rule): B2 reads a different
  timer from the one X came from.
- **7B-4, X is derived at this step's base, not at step 0.** Step 0 did record the
  population split: `SGL_CLOSED_ENTRIES` / `SGL_OPEN_ENTRIES` / `SGL_OWN_ENTRIES`
  12,498,190 / 151,140 / 2,420,676, and `scan_cone` 2,669.3 ms
  (`docs/measurements/2026-10-18-issue-160-step0.md:118`, `:137`). X needs more than that,
  for three reasons:
  - X is old cost minus NEW cost. The new cost (memo builds per distinct callee cone,
    `fold_row`, the projected region) depends on how many distinct callee SCCs are read and
    how large their closed sets are, and no step-0 counter records either.
  - Step 0 ran on Windows, at a base before steps 1-7a. 7a changed the recursive branch
    that half of X comes from.
  - The bench that applies X runs on server1 (Linux), at this base.
  The row's "re-derived at step 0" is read as "derived from a measurement at the step's own
  base, written down before code", which ledger P15 is.

## 10. Risks and open questions

1. **Server1 is shared.** One disturbed run raised a band 5.76-fold (Q band 735.9 →
   4,241.6, P3). Under §8 a void bound is re-run once and then fails. A disturbed bench can
   therefore drop a correct step; the epic's rule accepts that cost.
2. **X is a projection, and B1's margin is the band.** The probe placed the new path before
   the real one in each member loop, as the head runs it; it is not a "cold" reading. X now
   projects the WHOLE new region, so the expected saving is about X itself, and B1 passes
   by roughly Bq. Things that can make the head slower than the projection:
   - the head's function call and census overhead;
   - cache effects that differ with the old path gone;
   - probe 2's pre-warm: its `own_keys` and `hash` passes ran just before `sgl_full` and
     read the same data, which the head will not do (opus N1). Probe 2's memo sub-timer
     (median 585.9 ms) is not below probe 1's (510.3 ms), which had no such passes, so the
     effect looks small. It is not quantified.
   Things that can make it faster:
   - the census `add` calls inside the probe's regions.
   X already credits the old path's untimed remainder (382.2-742.3 ms per base run, P4):
   `own_keys` (301.5-380.7 ms measured alone, P18), dropping `best`, and loop overhead. That
   remainder is inside the parent's Q, and the projected head side (`sgl_full`) holds none of
   it, so it adds no extra margin.
3. **T2 needs a guard fixture.** It reuses the helpers 7a's T4 and `param_guard`'s tests
   use; if a contradicting-literal guard cannot be stated with them, T2 states the guard
   by assignment (CLAUDE.md: hand-state the precondition) and records how.
4. **Policy's `Both` build** now pays memo builds in addition to the `best` scan; part 3
   reports its wall.
