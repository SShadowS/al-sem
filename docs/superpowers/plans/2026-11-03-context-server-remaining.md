# Context server: the remaining work (plan, revised after code review)

**Status:** agreed plan, 2026-11-03. Drafted by Claude, reviewed and revised by Fable against
master `447e98c2` (the review notes are §7), owner decisions recorded below.

**Owner decisions (2026-11-03):**
- **P (stage 6, the declaration pack): not done.** Its measured ceiling is ~0.8–1.0 s of a
  ~34 s cold start (§2 P). The number is recorded; the pack spec stays as written for later.
- **S (the finer dependency re-resolution rule): done**, behind the recording facade and its
  tripwire test (§2 S), in place of P.
- **L6 (lazy raw emission for primary subjects): done**, as a recorded amendment to the
  design's eager-emission decision (§6.10) and the T-order fill expectations; measure the eager
  emission's share of the cold fold first.
- **L4b (cross-revision per-key tree sections): decided after L4a's numbers**; expected to stop
  at L4a.
- **W2 (an owned explain-index artifact): measured on the resident first**, built only if
  `Indexes` + `CrossRules` cost ≥ 0.4 s.

Build order with these decisions: W → L5 (+ L1's measurement) → L4a with L3 → L2 → S → L6;
I (#367, an `alsem analyze` item) in parallel at any point, outside `src/engine/resident/`.
**Design it extends:** `docs/superpowers/specs/2026-11-01-context-server-design.md` (stages 0–5b
as built). Every item keeps that design's contract: **every answer equals the CLI's output for a
cold build, byte for byte**, after every change type (B body, S declaration, F file, P package).
**Out of scope:** the Windows run (`scripts/windows-ctxsrv-check.ps1`).

Line numbers below are from master `447e98c2`.

## 0. Where we are

Measured on CDO (cross-app, release-fast, quiet machine), stage 5b (spec §6.11 as-built):

| After | refresh | fold | L4 | tree base | explain | prove | digest | query |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| cold | 18.0–18.8 s | 14.3–14.9 s | 0.58 s | 1.47 s | 6.1–6.3 s | 5.7–6.1 s | 3.2–3.4 s | 40 ms |
| B edit | 3.1–3.3 s | 2.16–2.25 s | 0.60–0.67 s | 1.45 s | 6.0–6.1 s | 5.8–6.3 s | 3.5 s | 37 ms |
| S edit | 10.4–11.2 s | 2.27–2.33 s | 0.64 s | 1.46–1.55 s | 6.3 s | 5.9–6.2 s | 3.3–3.8 s | 36 ms |
| P change | 19.5 s | 15.2 s | 0.58 s | 1.50 s | 6.2 s | 5.9 s | 3.5 s | 40 ms |

Memory: default ~6.0 GB RSS after a B edit (budget now 8 GiB); `--low-mem` ~4.8 GB.

Added by this review (cold one-shot `alsem` on CDO, this binary's Oct 31 release-fast build,
22 cores, two rounds each, `ALSEM_TRACE=1`): `digest.fingerprint_indexes` **93–101 ms**,
`digest.roots` 2.80–2.98 s (one root, `IssueReminder`), `snapshot.derive` 2.48–2.58 s (one
whole tree), `context.capability_cones` 5.5–6.9 s, model build (`crossapp.base_total`)
13.5–13.8 s, whole run 26–28 s, peak RSS 3.4 GB.

Where the remaining time goes:

- **Per heavy answer:** root witnesses (`digest.roots`) 2.8–3.2 s, recomputed by explain, prove
  and digest for the same root; explain's own render ~0.75–0.9 s (the spec's "`explain.indexes`
  0.9 s" and the cold run's whole `job.0.explain` 0.74 s disagree, so split them before acting);
  prove's post-root work 0.70–0.74 s (`prove.literal_reach`, `prove.group_paths`,
  `prove.reach_effects`); prove's det harvest and run ~1.5 s, **paid once per revision** (they are
  `Det`/`Run` artifacts), not per answer; per-view tree work 0.65 s (evidence indexes and
  capability facts) + 0.85–0.92 s (pd projection), paid once per distinct subject set per
  revision.
- **Per edit ("analysis prepared"):** fold 2.2 s (`pd.solve` ~1 s, of which the canonical
  finalize 0.58–0.74 s; `cone.direct` 0.33 s; row copies 0.19 s), L4 0.6 s, tree base 1.45 s
  (typed edges 0.52 s, call-site resolutions 0.48 s, order frames 0.22 s). S adds ~4.5 s of
  whole dependency re-resolution.
- **Cold start and P:** model build ~12–14 s, fold ~15 s (the one-shot composition is 5.5–6.9 s;
  the difference is mostly the eager raw emission for every primary subject, spec §6.10),
  so ~34 s to the first heavy answer.

## 1. What the code says (answers to the open questions)

**Q1. Is a root's `DigestEntryResult` a function of the root and options only, or of the view's
subject set?** Of the root and options only, as long as the root is one of the tree's subjects.
`digest_one_root` (`src/engine/l5/digest.rs:3868–4489`) reads: the root's own facts
(`idx.facts_by_routine[rid]`: its direct rows plus its inherited rows, each subject's slice
sorted by a fixed key in `derive_capability_facts`, `snapshot.rs:2422`); every routine's
**direct** facts through `CarrierCache` (`digest.rs:1309–1356`, over `direct_facts_by_op_kind`),
which a view carries for every covered routine whatever its subjects (`compose_tree_over`,
`snapshot.rs:1960`: "direct rows for every covered routine, inherited rows for the subjects");
the whole sections (typed edges, coverage, identities, event declarations, call-site resolutions,
boundary) shared by every view; the evidence indexes (per view, but "referenced-only": a wider
view only adds rows for other subjects' references); the pd arena scoped to the subjects' state
closure (a wider view keeps a superset; the root's own states and their closure are the same);
and `compute_ordering`'s reads of `root_classifications` (whole). Two existing tests pin exactly
this: `t3_digest_scoped_equals_unscoped` (`src/engine/l5/digest_rule_b_tests.rs:738`) and
`prove_scoped_equals_unscoped` (`:1030`), over the r0 corpus and the dependency fixtures, and
`digest_effects_each`'s doc states it as a contract (`src/engine/l5/digest_cli.rs:1237–1241`).
**So the memo key does not need the view.** The remaining inputs are fixed on the served path:
`return_summaries` is always `None` and `ordering_witness_only` always `false`
(`compute_digest_effects_cli_in`, `digest.rs:4927–4973`); `isolated_event_ids` is a function
of the model (`:4957`), so of the revision; `BoundaryMode` is `On` from `read_tree_shared`
(`src/engine/session.rs:938–946`) and `digest_effects_each` flips it to `Off` exactly for a root
inside a dependency (`digest_cli.rs:1297–1328`), so it is a function of the root. The only
request option that changes a root's entry is `include_reads`.

**Q2. Key on `Tree(subjects)` identity or on a content fingerprint of the view?** Neither:
key on **(root stable id, `include_reads`)** within the revision's store. The store is owned by
the revision (`Revision.store`, `src/engine/resident/revision.rs:231`), so "revision generation"
needs no place in the key; the store's own per-key generation (`store.rs:363–406`) already
guards evict-during-fill. Two requests with different subject sets that share a root share the
entry (Q1). The entry still needs *a* tree holding the root to be computed over: the request's
own `Tree(scope)` (its scope always contains the request's roots, `needs_of`,
`revision.rs:492–543`).

**Q3. Which per-view sections qualify for per-key carry (spec §6.11)?** Reading the derivers:

- `capability_facts` (`derive_capability_facts`, `snapshot.rs:2422–2550`): per id, the direct
  rows (cone product) plus the subject's raw rows. The rows of every **non-subject** routine are
  identical across views; a view re-interns and re-sorts all of them (`SnapTablesBuilder`,
  `:2485–2515`). Qualifies, split as a per-revision direct table plus per-subject slices.
- `operation_index` and `callsite_index` (`derive_operation_evidence` `:2977`,
  `derive_callsite_evidence` `:3104`): per view they walk **every workspace routine** and call
  `compute_routine_order(r)` (`:3022`, `:3141`; it rebuilds a `PFeatures` and walks the
  statement tree, `:1434`), then keep only the "referenced" sites. The candidate rows per
  routine are a function of that routine's row; the referenced set is direct witnesses (view
  independent) + typed-edge anchors (view independent) + the **subjects' inherited** witnesses
  (per view). Qualifies as per-id candidate rows with a per-view filter.
- `pd_solution` (`derive_pd_arena`, `:2780–2920`): does **not** qualify as a per-key section;
  its states index positions in the view's `capability_facts` and `typed_edges`, and its kept
  set is the subjects' closure. But most of its cost is view independent: `edge_ix` over every
  typed edge (`:2828–2844`), `direct_fact_ix` over every fact (`:2846–2851`), and a scan of all
  2.8 M states (`:2855`). Those move to the per-revision tree base.

So the first step for L4 is not the spec's cross-revision rule at all: it is hoisting
view-independent work out of the per-view derivation into `TreeBase` (per revision). See L4a.

**Q4. Does the ordering cell still build a private snapshot after stage 5b?** Yes, and it is
not the tree store. `ArtifactKey::Ordering` is filled by `dc::fill_ordering(model)`
(`revision.rs:573`) → `compute_ordering_facts` (`src/engine/l5/detector_context.rs:1196–1210`,
`ordering_facts.rs:218`) → `compute_digest_effects_for_ordering_with` → **`compose_snapshot(resolved)`**
(`digest.rs:4806–4809`), the whole composition with raw for every primary subject
(`snapshot.rs:1569`; its doc records 46 GB / 60 s on cross-app CDO before the cone-memory fix).
`ORDERING_RELEVANT` is at `digest.rs:3966–3977` (the issue's `~3921–3932` has drifted). **But no
served command ever fills it:** d47/d49/d51 are opt-in detectors (`src/engine/l5/detectors/
mod.rs:1976–2040`, `opt_in_detectors`), prove and digest run `default_detectors()`
(`src/engine/l5/prove.rs:1384`, `digest_cli.rs:2424`) and expose no detector flag, and explain
and query run none. #367 is therefore an `alsem analyze` item, not a context-server latency
item. Its saving also has a better shape than fact kinds: `ordering_kept_roots` (`digest.rs:4874`)
drops roots without an IO/UI ingredient *after* the snapshot is built; on single-app CDO
`ordering.roots_all` = 5,479 against `roots_kept` = 1,350
(`docs/measurements/2026-10-18-issue-160-step0.md:325`), so emitting raw only for kept roots
removes ~75 % of the subjects before any kind filter.

**Q5. Is W safe to build before L4?** Yes. W keys on the root (Q2) and depends on the Tree
artifact only as a prerequisite and as the input of its fill. L4a changes how a view's tree is
derived, not its content (the per-view output stays byte-equal, which L4a's own proof pins), nor
its key (`Tree(scope)`, `revision.rs:73`). Nothing in W reads the tree's internals.

Two more facts the plan relied on and that need correcting:

- The plan proposed memoizing `FingerprintIndexes`, `OrderingIndex` and `CarrierCache` per
  (view, `BoundaryMode`). They hold borrows into the snapshot (`FingerprintIndexes<'a>`,
  `digest.rs:184–220`; `HopCache` keyed by edge address, `:1243`; `CarrierCache<'i>` of `&'i str`),
  so storing them beside an `Arc<CapabilitySnapshot>` needs an owned, index-based rewrite. The
  measured cost they would save is **0.1 s per answer** (`digest.fingerprint_indexes`
  93–101 ms). Not worth it; dropped.
- `digest_effects_each` **mutates** the entries after the root pass: prove's literal-reach pass
  sets `literal_unreachable`, `dead_group_resources`, `group_resource_paths` and prunes
  `via_paths` (`digest_cli.rs:1330–1375`), and `run_digest_query_full_from_entries` moves group
  resources out of entries (`:1428`, "moved, not copied"). A memo must hand out **clones**, never
  the stored value, and the memoized value must be the pre-pass entry.

## 2. The work items

Each item: its own branch; task gate per commit; `check-goldens` and `cdo-gate` before
reporting; cold-oracle tests with discrimination proofs recorded (break with Edit, run, revert,
run); its CDO measurement on a quiet machine via `aldump --session-refresh <copy> --verify`
plus the spans named. No item may move a golden.

### W — a per-revision memo of root witnesses (new; not in the spec)

**Problem.** Each of explain, prove and digest about the same routine rebuilds that root's
`DigestEntryResult` (`digest.roots` 2.8–3.2 s on CDO, single-threaded for one root). An agent
that edits a routine and then asks all three pays it three times per edit.

**Design.**

1. *Split the pure half from the per-request half of `digest_effects_each`
   (`digest_cli.rs:1246–1424`).* New `root_entries(tree, model, roots, include_reads, boundary)
   -> Vec<Option<DigestEntryResult>>`: the root partition (workspace / dependency, `:1297–1328`),
   the two `compute_digest_effects_cli_in` calls and the sort — nothing else; `None` for a root
   that yields no entry (not reportable, no display), so a miss is stored and not retried. The
   rest (coverage graph, prove's literal reach, unresolved cones, the `DigestEffects` assembly)
   becomes `finish_effects(tree, model, readers, entries_by_root, …)` taking owned entries. The
   one-shot path calls both in sequence; its bytes do not move (goldens, T-order).
2. *The artifact.* `ArtifactKey::Witness { root: String /* stable id */, include_reads: bool }`,
   kind `"witness"`, **phase 3** (new; `ensure_prepared` iterates `0..=last_phase`, `store.rs:
   495`, so only `StoreKey::phase` changes). Value `ArtifactValue::Witness(Arc<Option<DigestEntryResult>>)`.
   Prerequisite: the request's `Tree(scope)`. `needs_of` adds one key per root of every
   `TreeRead::Effects` demand, with that demand's `include_reads`. The fill asserts the fixed
   options (`return_summaries` none, `ordering_witness_only` false) so a future caller with other
   options cannot alias the key.
3. *Batch fill.* The store fills claimed keys one at a time (`store.rs:560–590`), and each fill
   would rebuild the indexes (0.1 s) and lose the per-root parallelism of `digest_query_with`
   (`digest.rs:3524`). So the fill closure in `serve_plan` (`revision.rs:967–977`) captures a
   request-local `Mutex<BTreeMap<ArtifactKey, Arc<Option<DigestEntryResult>>>>`. The first
   `Witness` fill of a request computes `root_entries` for **every** root of the request's tree
   demand whose key is absent from `cx.snapshot`, stores them all in the scratch, and returns its
   own; later fills of the same request return from the scratch. A root claimed by another
   concurrent request may be computed twice; the store keeps the first publication (gen check,
   `store.rs:655–690`).
4. *The read.* `trees()` (`revision.rs:840–880`) passes `finish_effects` clones of the memoized
   entries (`(**arc).clone()`), never the `Arc`'s contents.
5. *Budget and eviction.* `"witness"` goes **first** in `EVICTION_ORDER` (`revision.rs:431–440`,
   before `"raw"`); `Case::Fill("witness")` gets its own estimate once measured (the default
   `Fill(_)` is 60 MiB, `budget.rs:137`; expect far less); `reclaim_kind("witness")` joins the
   tree reclaim at refresh (`session.rs:507–514`); the census parts and `kind()` names gain the
   kind. Pins and holds need no change (`ensure_prepared` pins the needs; `hold` keeps them
   through the compute).
6. *Not done:* carrying witnesses across revisions. A root's entry depends on every routine its
   witnesses cross (typed edges, call-site evidence, direct facts of carriers, the pd closure),
   so invalidation needs a reachable-set rule. Measure how often an agent re-asks a root across
   an edit before designing it.

**Tests.**

- T-order extension (`src/engine/resident/revision_tests.rs:181`,
  `mixed_orders_over_one_revision_equal_cold_runs`): all six orders of
  `[Explain, Prove, Digest]` on one root, plus `[Digest, Digest --include-reads, Digest]`, each
  equal to cold; fill counts: `witness` fills = number of distinct `(root, include_reads)`
  pairs (1 for the six orders, 2 with `--include-reads`), `tree` fills unchanged.
- Cross-view hit: explain over three roots, then prove over one of them: the prove fills no
  witness; equal to cold. Add the same pair to the CDO measurement protocol (it is the case the
  small fixtures under-exercise).
- Cold oracle after B/S/F/P (`incr_tests.rs:95`, `assert_answers_equal_cold`): unchanged tests,
  now also proving that a new revision's store starts empty (the post-edit answer equals cold).
- Concurrency: two requests on the same root at once (the existing
  `concurrent_requests_and_evictions_answer_the_cold_bytes`, `revision_tests.rs:464`, extended
  with witness keys): both equal cold; eviction of a witness between claim and publish discards
  the value (gen mismatch) and the key refills.
- Discrimination proofs (each must be seen to fail, then pass): (a) drop `include_reads` from
  the key → `digest --include-reads` after a plain digest has no `DB_READ` effects; (b) hand the
  read the stored entry instead of a clone → a digest after a prove on the same root shows
  prove's pruned `via_paths` / `literal_unreachable` (differs from cold); (c) drop the `Tree`
  prerequisite → the fill fails loudly ("a tree is not published"), never a wrong answer;
  (d) store the entry only for the computed root and drop the batch scratch → the fill count for
  a three-root explain is 3 and `digest.fingerprint_indexes` runs 3 times (a counter test, not a
  byte test).

**Measurement and target.** Per answer: `digest.roots` + a new `witness.hit`/`witness.miss`
counter in `status`. Expected on CDO for the second and third heavy answer on the same root:
digest 3.4 s → ~0.4 s; prove 5.9 s → ~2.9 s (first prove in a revision still pays det+run
1.5 s; a later one ~1.3 s); explain 6.1 s → ~3.1 s. Memory: measure the bytes of one entry
(`IssueReminder` and a hub root); expect KB to a few MB per root.

**W2 (optional, measured first).** Explain's own post-root work: split `explain.indexes` from
`explain.cards`/render on the resident (the cold run shows 0.74 s for all of it, the spec's
note 0.9 s for the indexes alone). If `Indexes::new(model)` + `CrossRules::new(model)`
(`explain.rs:703–707`) are ≥ 0.4 s, they are a function of the model alone and belong in a
per-revision artifact; both borrow the model (`Indexes<'m>`, `explain.rs:471`;
`CrossRules<'m>`, `cross_rules.rs:214`), so that needs an owned form (row indices and owned
strings) — a contained refactor, done only if the number justifies it.

### L5 — direct facts: parallel first, memo second

**Problem (corrected).** `det_cone_inputs_over` (`src/engine/l5/detector_context.rs:1353–1393`)
computes `direct_facts_for_routine` for every routine of the model in a **serial** loop
(`:1374`), every fold: `cone.direct` 0.33 s after every edit, and on cold start.

**Design.** Step 1: `par_iter` over the routines with an order-preserving collect (each call is
pure over the row, its publisher events and its uncertainty reasons); the merge per id
(`projection::merge_per_id`) is unchanged. Expect ~0.05 s on many cores. Step 2 only if step 1
leaves > 0.1 s: memoize per routine across folds keyed by `(row digest, publisher-events
digest)`; `pd_input_digests` (`capability_cone.rs:3825`) already hashes the rows per id, so the
row half of the key exists. The plan's "function of one routine's row" was incomplete: the
inputs are the row, `publisher_events_by_routine` (the event graph) and
`callsite_local_coverage_reasons` (the relation's uncertainty edges); the memo covers the first
two and the fold still folds the third (cheap).

**Tests.** `direct_full` and `coverage_in` byte-equal (hash both maps) between the serial and
parallel paths on CDO; goldens; discrimination: reverse the collected order → the hash moves.

### L1 — per-producer merge instead of the global canonical re-sort

**Problem (sharpened).** `pd.finalize` (`src/engine/l4/pd_solve.rs:394–523`) is 0.58–0.74 s of
a B-edit fold. It sorts all producers, all edges and all ~2.8 M states, then runs one pass that
remaps links, sorts and dedups each state's links, and rebuilds `by_routine` and the resolved
entries (`:470–521`). Only the state sort is what a merge removes; the pass stays.

**Design.** First add two spans (`pd.finalize.sort`, `pd.finalize.build`) and measure. Proceed
only if the sort half is ≥ 0.3 s. Then: copied producers' states arrive one contiguous range per
producer in the previous canonical order (`:820–905`), and the within-producer rank
`(dist, routine internal id in string order, t, reqs)` is preserved from one solve to the next
(string order is global; adding or removing routines does not reorder the rest), so a copied
range is already sorted. Sort only each fresh producer's states, order the producers by
`producer_cmp`, concatenate, rank edges as today (far fewer than states), and run the existing
build pass. States never link across producers, so the callee remap is a per-range offset.

**Tests.** Stage 1b's shuffle tests (`T-arena-canon`, `pd_solve.rs:1759+`) and
`fold_reuses_untouched_producers_and_equals_cold` (`incr_tests.rs:206`); the merged arena `==`
the full finalize's on CDO (the canonical form makes this a plain equality). Discrimination:
skip the sort of one fresh producer's states → the arena differs.

### L4 — per-view tree work, in two steps

**L4a (per revision; the new first step).** Hoist the view-independent parts of the per-view
derivation into `TreeBase` (`snapshot.rs:1836`, `derive_tree_base` `:1900`):

- per-routine evidence candidates: the `compute_routine_order` output (operation sites, call
  sites with order and anchors) computed once per revision (today three times per view + once in
  `derive_routine_order_frames`, `:3799`); the view-independent half of the referenced set
  (direct witnesses, typed-edge anchors); per view: add the subjects' inherited references and
  filter;
- the direct-fact table (`SnapTables` for every covered routine's direct rows, sorted per
  subject) and `direct_fact_ix`; per view: splice the subjects' slices (direct + raw, re-sorted
  for those subjects only) and fix up positions;
- `edge_ix` over the typed edges (`:2828–2844`); per view only the closure walk and the
  per-kept-state mapping (`:2855–2905`).

The view's output must stay byte-equal to today's (`compose_tree_over` is the oracle; keep the
old path behind a test-only flag until the equality test on CDO passes, then delete it).
Expected: the per-view 0.65 + 0.9 s falls to the subjects' own work (tens of ms for one root);
the tree base grows by what moved in (measure `tree.base`; the per-edit cost may rise by
~0.1–0.3 s, but it is paid once per revision instead of once per view).

**L3 (with L4a; same construction sites).** Share the whole sections as `Arc`s inside a view's
tree instead of cloning them (65 ms per view; a revision with one view holds them twice).
Remaining `Vec` fields of `CapabilitySnapshot` (`snapshot.rs:1176–1200`): `typed_edges`,
`callsite_resolutions`, `analysis_gaps`, `event_declarations`, `root_classifications`,
`routine_order_frames` (`identities` and `coverage` are `Arc` already). 27 struct-literal
construction sites (`grep -c 'CapabilitySnapshot {'`), plus assignment sites. Measure the memory
with the census (`snapshot.*` buckets) before and after.

**L4b (across revisions, per key; only if measured to matter).** The spec's §6.11 rule, applied
to the per-row candidates of L4a and to the per-row halves of the tree base's whole sections
(typed edges 0.52 s, call-site resolutions 0.48 s, order frames 0.22 s per edit): carry each
id's candidate rows keyed by its row digest, rebuild the id's rows when its file is in Δ_files,
its row digest moved, or it is in Δ_pd_output (the "full row including anchors" rule). The
whole sections' global sorts stay whole. Decide after L4a's numbers.

**Tests.** The view equality above on CDO and every fixture; `T-tree`/`T-typed-edges`/
`T-callsite-res` (spec §11) for L4b; discrimination for L4a: leave one subject's inherited
references out of the per-view filter → a callsite row a witness needs is missing and the answer
differs from cold.

### L2 — parallel fresh-producer solving (cold and P)

**Problem.** Cold `pd.solve` 3.7–4.2 s (every producer fresh); P the same.

**Design.** The BFS per producer is independent: the state key includes the producer
(`pd_solve.rs:776`), links join states of one producer only, and `finalize` re-ranks everything
afterwards. Shared pieces today are the name interner, the edge interner (`edge_ix`) and
`arena.states` used as the queue (`:905–985`). Solve each fresh producer on its own worker into
local `(states, raw_links, edges-by-address)` with local indices; concatenate; dedup edges by
address; `finalize` as today. Expected on 22 cores: ~3 s → well under 1 s; also cuts the P
refresh.

**Tests.** Equality of the finalized arena to the serial solve (CDO and fixtures); shuffle
independence; discrimination: drop the edge dedup → duplicate edges → `finalize`'s
`debug_assert` on edge identity fires / the arena differs.

### S — the finer dependency re-resolution rule (stage 6's other half; new in this plan)

**Problem.** An S edit (a signature or declaration change — the most common edit after a body
edit when an agent adds a procedure) costs 10.4–11.2 s of refresh, of which **4.0–4.7 s** is
re-resolving every dependency body whole (spec §6.6; stage 4 as-built). The spec defers a
finer, object-level rule to stage 6 behind "a tripwire test proving no read bypasses its
recording facade". The plan omitted it; it is worth more than the pack.

**Design (measurement and audit first).** (1) Record, per dependency body resolution, which
workspace declaration surfaces it reads (a recording facade over the index reads:
implementers, extensions, name uniqueness, the ABI). (2) The tripwire: a test that every index
read in dependency-body resolution goes through the facade (a counting hook; a direct read
fails). (3) The rule: under S, re-resolve a dependency body only when a surface it recorded
changed; else keep its edges. (4) Proof: after every S case of `T-incr`, the dependency edges
equal a whole re-resolution's. Owner's call: the spec judged a precise key fragile; the facade
and tripwire are what make it safe.

### I — issue #367: the ordering cell's inputs (an `alsem analyze` item)

**Problem (corrected).** See Q4: the cell's private snapshot is `compose_snapshot` of the whole
model; no served command fills it; the issue's line references have drifted.

**Design.** (1) Measure `context.ordering_facts` and its `ordering.compose_snapshot` span in
`alsem analyze` with d47/d49/d51 enabled, cross-app on CDO, after the cone-memory fix (facts are
~10 bytes each now), with `ordering.roots_all`/`roots_kept`. (2) If it pays, the first lever is
**subjects, not kinds**: compute the kept-root ingredient test from the direct facts and cone
products (the same ops `effect_type_of` grades) before raw emission, and emit raw only for kept
roots — the tree-store machinery (`Raw(scope)`, `Tree(scope)`) already supports a subject
restriction. (3) Only if event-publish rows still dominate the kept roots' raw, the kind
restriction as filed, on the cell's own build, never on the shared snapshot; an unbuilt kind
read fails loudly. (4) Guard for the server: if a served command ever demands
`ORDERING_FACTS`, its `Ordering` fill must read the revision's fold and a tree over the kept
roots, not `compose_snapshot` (today that would be a whole second composition).

**Tests.** d47/d49/d51 findings on CDO at both scopes byte-identical; discrimination: drop a
kept root → a finding disappears. Or close #367 with the numbers.

### P — stage 6: the declaration pack (owner's call; small ceiling)

**Problem (corrected).** The pack persists declaration products and `RoutineMeta` per app; bodies
are re-parsed and resolutions never persisted (pack spec §6, context spec §6.4). What it replaces
is the dependency layer: `preflight.dep_layer` **0.93–0.99 s** of a 13.5–13.8 s cold model build
(this review's runs; spec §1.2: 0.98–1.17 s), minus the pack load (the format gate measured
~70–110 ms for Base Application's pack, `docs/2026-08-07-dep-pack-gate-measurement.md`). Parse
(1.2–1.4 s) and dependency-body resolution (4.1–4.5 s) stay. **Ceiling ≈ 0.8–1.0 s per cold
start or P change**, against a ~34 s cold start. On the resident, B/S/F never rebuild the tier.

**Design (if the owner wants it).** Pack spec §14 steps 4–6: `EXTRACTION_FINGERPRINT` and its
canary (§8), the seam in `build_dep_layer_cached` (`src/program/build.rs:118`) as a third route
beside parse-and-extract and `ingest_abi` (§5, both rewires of §11), the cold-vs-warm
output-equivalence gate (§12). The format and serde exist (`src/program/pack/mod.rs`,
`PACK_SCHEMA` 12). Key components per §7; a discrimination proof per component (flip one → the
pack is a miss). Parity: every answer and diagnostic from packs equals fresh; stale and corrupt
packs rejected; concurrent writers safe.

**Decide after measuring:** state the fresh-minus-pack number end to end (wall and peak) and
stop if it is under ~0.5 s.

### L6 — candidate: lazy raw emission for primary subjects (cold start and P; owner's call)

The resident's cold fold is 14.3–14.9 s where the one-shot composition for one root is
5.5–6.9 s; the spec's census attributes the difference largely to the eager raw emission for
every primary subject (§6.10 "Raw emission for primary subjects eagerly"; singleton walks
6.7 s with raw scans 2.5 s and emission 2.4 s). Emitting a primary subject's raw on demand (as
dependency subjects already are, `Raw(scope)`) would cut cold start and P by several seconds at
the price of a per-subject emission on a root's first ask (ms for a primary routine). It changes
a design decision and the T-order fill expectations, so it is listed for the owner, not
scheduled. Measure the eager emission's share of the cold fold first (`cone.walk` and the raw
spans on a cold refresh).

## 3. Order and dependencies

1. **W.** Biggest per-answer win, self-contained (`digest_cli.rs`, `revision.rs`, `session.rs`
   needs, budget/eviction tables). Independent of L4 (Q5).
2. **L5 step 1** (a one-line parallelization) and **L1's measurement split** — together, a day.
   L1 proper only if the sort half is ≥ 0.3 s.
3. **L4a with L3** (same files and construction sites), then measure; **L4b** only on the
   numbers.
4. **L2** (cold and P; independent of 1–3; touches `pd_solve.rs` only, so it can run beside
   L4a).
5. **S** — owner's call; measurement and the facade audit first.
6. **I** — `alsem analyze` only; can run in parallel with anything above (it stays out of
   `src/engine/resident/`).
7. **P** — owner's call; last, because its ceiling is ~1 s.
8. **L6** — owner's call after the cold-fold measurement.

Items 1–4 touch `src/engine/resident/` and `src/engine/l4`/`l5`; run W, then L5/L1, then
L4a/L3 one at a time; L2 may run beside L4a. Each item lands on master through
`scripts/merge-and-push <branch>`.

## 4. Targets after the plan (hypotheses, to be measured)

| Measure | Now | After |
|---|---:|---:|
| 2nd/3rd heavy answer on the same root (same revision) | digest 3.4 s / prove 5.9 s / explain 6.1 s | ~0.4 s / ~1.3–2.9 s / ~3.1 s (W) |
| first heavy answer on a new root (same revision) | ~6 s | ~4.5 s (L4a: per-view tree work from 1.55 s to ≈ 0.1 s) |
| analysis prepared after a B edit | fold 2.2 s + L4 0.6 s + tree base 1.45 s | fold ~1.6–1.9 s (L5, L1) + 0.6 s + tree base ~1.5–1.7 s (L4a moves work in) |
| S refresh | 10.4–11.2 s | ~6–7 s (S, if the finer rule lands) |
| cold start to first heavy answer | ~34 s | minus ~3 s (L2), minus ~1 s (P), minus several s (L6) — measured |

## 5. Measurement protocol additions

`aldump --session-refresh <copy> --verify` (every step equal to cold) with `ALSEM_TRACE=1`,
reading: `digest.roots`, `digest.fingerprint_indexes`, new `witness.hit`/`witness.miss` counters
and `fill_counts()["witness"]`; `digest.coverage_graph` and `digest.unresolved_cones` per answer
(unmeasured today; this review's binary shows them under 20 ms or absent); `explain.indexes`
against `explain.cards`; `pd.finalize.sort`/`pd.finalize.build`; `cone.direct`; `tree.*` per
section before and after L4a; peak and resident RSS per step in both modes (default 8 GiB,
`--low-mem` 6 GiB). Quote numbers only from release-fast on a quiet machine; never from the
`cdo` profile.

## 6. Risks and interactions

- **Memory.** W adds one small value per asked root per revision, evicted first and dropped at
  refresh; L3 removes a copy of the whole sections per view; L4a moves per-view work into one
  per-revision table (measure; it replaces N view copies by one). Nothing here changes the
  cone store, so `--low-mem` (which keeps it, §6.10 as-built deviation 5) behaves the same; the
  new kinds must be in `EVICTION_ORDER` and in the refresh reclaim so the 8 GiB default and the
  6 GiB `--low-mem` budgets see them.
- **Aliasing.** The one real correctness hazard in W is handing a memoized entry to a pass that
  mutates it (prove). The clone rule and discrimination proof (b) are not optional.
- **The batch scratch** lives in one request; two concurrent requests on overlapping roots may
  compute a root twice. Acceptable; the store's generation check keeps one value.
- **L1 and L2 share `finalize`**; land L1's measurement first so L2's equality test is against a
  settled finalize.
- **`alsem analyze` is not served**; I's work must not be measured through the server.

## 7. Review notes (what changed from the draft and why)

- **W:** removed the view and the revision generation from the key (Q1, Q2: the root's entry is
  view independent, pinned by two existing tests; the store is per revision). Removed the
  proposal to memoize the indexes (they borrow the snapshot; they cost 0.1 s, measured here).
  Added the pure/impure split of `digest_effects_each`, the clone rule (prove mutates entries),
  `Option` values for roots with no entry, the batch fill (the store fills one key at a time),
  phase 3, the eviction/reclaim/budget wiring, and four discrimination proofs that can each be
  seen to fail. Gave per-command targets instead of "~3 s".
- **I:** corrected the problem statement: d47/d49/d51 are opt-in, no served command fills the
  ordering cell, the private snapshot is `compose_snapshot` of the whole model, the line
  references drifted. Re-shaped the fix as a subject restriction first (75 % of roots are not
  kept on CDO), kinds second; re-homed the item to `alsem analyze`.
- **P:** stated the measured ceiling (~1 s of a ~34 s cold start) so the owner can decide
  before any work; kept the spec's steps for the case it is wanted.
- **Added S** (the finer dependency re-resolution rule, 4.0–4.7 s per S refresh): stage 6's
  other half, omitted by the draft and worth more than P.
- **L4:** answered Q3 from the derivers; split into L4a (hoist view-independent work into the
  tree base; no cross-revision rule needed) and L4b (the spec's per-key carry), because most of
  the per-view 1.55 s is view-independent work redone per view (`compute_routine_order` for
  every routine, `edge_ix`, `direct_fact_ix`, re-interning every direct row). Folded L3 into L4a.
- **L5:** the loop is serial; parallelize before memoizing; the input set was incomplete.
- **L1:** the finalize span is sort + build pass; measure the split first; explained why copied
  ranges are already in canonical order.
- **L2:** moved above P (a bigger cold-start win, independent of other items).
- **Added L6** as a candidate for the owner (the eager primary raw emission dominates the cold
  fold).
- **Order:** W, L5/L1, L4a+L3, L2, then the owner's calls (S, P, L6) and I in parallel.
- **Measurements added by this review:** cold one-shot on CDO, two rounds each of explain, prove,
  digest (`/tmp/fable-measure/*.json`): `digest.fingerprint_indexes` 93–101 ms, `digest.roots`
  2.80–2.98 s, `snapshot.derive` 2.48–2.58 s, `preflight.dep_layer` 0.93–0.99 s, model build
  13.5–13.8 s, wall 26–28 s, peak 3.4 GB. The binary is the Oct 31 release-fast build (before
  stage 5b), so these are one-shot numbers, used only for the per-answer proportions the resident
  shares.
