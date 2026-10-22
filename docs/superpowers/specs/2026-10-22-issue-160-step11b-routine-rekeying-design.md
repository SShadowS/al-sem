# #160 child spec: step 11b — routine identity corrections, `RoutineIx` re-keying, the one collided-id rule

Status: revised after spec panel round 1 (2026-10-22; findings R1-01 to R1-22 in
`.agent/issue-160/findings.json`, conductor decisions 1-10 applied). Made from the pre-draft
`/work/bench/160-11b/draft-11b-design.md` once the probes had run. §0 lists what the probes
changed; §0.1 lists what round 1 changed.
Child of `2026-10-18-issue-160-analysis-session-design.md` (the epic): §1 goal 7, §5.3, §5.4,
§5.5 (M1, M2, M7), §5.11, §9 row **11b**, E2, E11, E14, I2, I6, I7, I9, I10, Decisions 6, 25,
39, 43, 57, 58. It builds on `2026-10-22-issue-160-steps10-11a-siteix-routine-identity-design.md`
("10+11a" below): its §4.6-§4.10, its amendments 11A-2 to 11A-4, its §5.10, and the two ledger
discoveries that name 11b (the identity corrections; A8's precondition). The shape follows
10+11a and `2026-10-21-issue-160-step7b-closed-sets-fold-memo-design.md`. Where this document
and the epic disagree, this document is wrong, EXCEPT for the numbered amendments of §9
(11B-1 to 11B-8). Each amendment gives its reason; until the epic takes it, it is an open
question for the panel.

Base: master `5c42b8f3`, worktree `/work/al-sem-issue-160-a5`. Compared with the 10+11a head
`3b199117`, it adds only the grammar bump #207 (`tree-sitter-al` `960ed312`; the lowerer, the
vocabulary, the cache stamp) and an agentflow change. No file this design edits has moved, and
the CDO histogram SHA and collision line did not change (P1). Every `file:line` below is in that
tree. Every fact the design rests on is a row of the ledger's `## Assumption probes` table
(`.agent/issue-160/ledger.md`, P1-P16). The probe patch, the scripts and the raw output are in
`/work/bench/160-11b/` (`probe-final.patch`, `run-base.sh`, `run-probe.sh`, `p6-inventory.md`,
and since round 1 `p6b-trace.md`, `identity-run.sh`, `idcheck.py`, `idrun-C/`).
The patch is reverted: `git diff -- src crates tests` is empty.

Words (the same as 10+11a, plus these):
- **Row** = one entry of `ModelEntities::routines`. **`RoutineIx`** = the row's position.
  **Node** = a `RoutineNodeId`. **`RowNodes`**, **J2**, **J5**: as defined in 10+11a.
- **Collision group** = two or more rows that share one string id (`ModelRoutine.id`). The rows
  of a group also share `stable_routine_id` and every call-site id `{id}/cs{n}`
  (`program_calls.rs:999, :1638, :2155, :2259`; `workspace.rs:1349-1355`).
- **Twin rows** = two rows of ONE node (an `#if`-split routine). They can share a string id
  (`GetPurchaseHeaderPageID`, `GetSalesHeaderPageID`: a collision group) or have different ids
  (`GetAccessToken`, `GetFreshAccessToken`: not a group).
- **Alias** = J2 answering node B with a row of a different node A, because the two nodes'
  declarations share an anchor. CDO has one: `AcquireTokenFromCache`.
- **Merge point** = a place where two rows of one string id meet in a store keyed by a string
  (§3.2: 22 from P6, plus what the round-1 trace found).
- **Projection** = the step that turns per-row producer outputs into the per-id values the
  existing computations and readers take. It runs BEFORE the computation (the cone, the solve,
  the event graph), never after it (§4.5).
- **Owning row** of a lookup key = a row that has that key: for `{id}/cs{n}`, a row of the id
  with at least n+1 call sites; for `{id}/op{n}`, the same with operations; for the id itself,
  every row of it.
- **Stage C / A / B** = the three parts of this claim, in that order (§4.1). Stage C is Task 1:
  committed and identity-checked on its own, and mergeable alone.

## 0. What the probes changed in the pre-draft

1. **R-MOVES is measured, not predicted** (P3, P4, P4c, P5). On CDO, stage C moves exactly 14
   cross-app SARIF results at each scope. All 14 are `d9-transaction-span-summary`, and in each
   one only the "publishes N event(s)" count changes, by +1. The alias correction moves no SARIF
   at all: the row whose edges it changes is reached by no primary routine. J5 moves nothing on
   CDO. Single-app CDO and all 246 fixture directories are byte-identical in both universes
   and at both scopes.
2. **The inventory has 22 merge points, not 9** (P6). It also found duplications that today's
   code makes for a collided id: k copies of its edges in the reverse graph and in r3a1, one
   event symbol per publisher row, doubled interface candidates, a temp rewrite that reaches
   only the last row, and a transaction-span seed row that is not the commit row. §3.2
   classifies all of them, and §4.5/§4.6 handle each class.
3. **Stage B is predicted to be byte-identical on CDO and on the corpus** (P12). Every collided
   row on both has 0 direct facts and the same coverage, and the rows differ only in their
   edges, which are already unioned. So the "one rule" has no natural witness. Its evidence is
   the hand-stated fixtures of §5 (a stated limit, F10).
4. **The `unmapped` branch of the event graph is dead after C** (P13: every one of the 1,087
   CDO subscriber rows has a node, and so does every fixture row). D7 keeps the branch as a
   fail-closed guard and keeps A8 discriminating by setting `None` by hand (§5.5).
5. Also: the epic row's "byte-identical" survives everywhere except those 14 results (11B-1
   narrows to that). Q1 (fan-out) has its count: 10 routes (P10). The coverage order exists
   (P8). One fixture, `lsp-diff-deps`, has a J5 gap, and correcting it moves nothing (P2, P4c).

### 0.1 What spec panel round 1 changed (R1-01 to R1-22; conductor decisions)

- **One claim; C is Task 1**, committed and identity-checked on its own, mergeable alone (Q7).
- **Stage A shrinks (R1-05).** The cone and the L4 solve stay computed per string id. A gives
  the PRODUCERS per-row outputs and adds the projection that turns them into today's per-id
  inputs BEFORE the computation; B changes only that projection. Per-row cones are deferred to
  step 6 (discovery, §12). T1's per-row derived-row assertion is gone. `from_ix` goes on
  `CallEdge` only (R1-16).
- **The union (R1-01)** applies only across the rows of ONE collided id; a single-row id passes
  through untouched. Its identity is the whole payload (full-value dedup), and ties are broken by
  full value, so the result does not depend on row order.
- **The single-valued pick (R1-04)** is the LAST row that OWNS the key, per key: a call site's
  edge and its bindings come from that one row; the within-row candidate policy (R9's first
  implementer inside one site) is kept.
- **Guard frames (R1-02) keep today's semantics** (per id, LAST row). The defect (alternative
  arms' requirements are conjunctive) is a discovery for step 6 (§12).
- **`event_param_temp` (R1-03)** inspects each raise at `(RoutineIx, site)` and suppresses only
  when EVERY applicable raise proves temporary; a mixed physical/temp negative fixture pins it.
- **`rows_of_id` is transient (R1-12)**, built inside the projection from the current rows, so a
  hand-stated collision cannot leave it stale. The rule's fixtures use natural `#if` collisions
  where they can, and each asserts its group size first.
- **The trace (R1-08)** read every path P6 left untraced (§3.2's new rows), and an order-swap
  test (T2) was added (narrowed in round 2 to set-valued surfaces, §0.2).
- **Fallbacks bounded (R1-10)**, **the identity run fail-closed (R1-11)** (new
  `identity-run.sh` / `idcheck.py`, re-run on the prototype: §3.4), **JSON compared beside
  SARIF (R1-21)**, **memory owned and counted (R1-09)**, **11B-8** (Decision 25, epic
  §5.3/§5.4/§5.9; R1-13), Q1-Q7 decided (§11), discoveries for step 6 (§12), full module
  paths (R1-14), the absent-J2 and `node_of == None` behaviour stated (R1-19, R1-20), C's
  leftovers listed (R1-22).

### 0.2 What spec panel round 2 changed (R1-01, R1-04, R1-08, R1-11 re-raised; R2-01 to R2-04)

- **Class U's collided lists get the full-value tie order** (R1-01): only where two or more
  rows of one id contribute; pinned through production `edges_by_from` by T2u.
- **The per-site consumers that bypass R9 go through one pick** (R1-04): `d37` and
  `compose_roles_only` read `site_pick` (edge AND bindings from the LAST owning row; §4.6; T6b;
  F11 if a single-row answer would move).
- **A parameter's role and dispatch are single-valued** (R2-01): class P per
  `(id, parameter index)`, LAST owning row, never a union (T1j).
- **T2 is narrowed** (R1-08, R2-03): it compares set-valued surfaces only, with the arms'
  anchors normalized; classes P, R and G are excluded by design; §4.7's "the pick cannot show"
  is withdrawn; there is no claim of universal coverage. R4b and policy get production witnesses
  (T1b, T1i). `p6b-trace.md`'s two unchecked paths are closed.
- **R-MOVES is pinned by identity** (R1-11): `expect-C-exact.json` holds rule, fingerprint and
  location per moved result. Re-run on the saved reports: 0 violations. A negative substitution
  with another `d9` result fails as it must (§3.4).
- **T9 runs through the production projection** (R2-02): `should_panic` in debug, the census
  counter in release; the mutation removes the projection's call.
- **No serialization tie-break** (R2-04): a total order (`Ord`) per payload type, asserted at
  Task A (§4.6).

## Feasibility first

| Item (epic row 11b + the 11b discoveries) | Verdict | Evidence (ledger row) |
|---|---|---|
| Discovery: `j2_alias_nodes` = 0 and `j5_compat_rows` = 0 on CDO | **feasible**, measured on a prototype | under the stage-C prototype the CDO line reads `j2_alias_nodes=0 j5_compat_rows=0` (P4c) |
| Discovery: the 5 `caller_mismatch` sites leave R-RESIDUAL | **feasible**, measured | anchor and node checks disagree on exactly those 5 and nowhere else (P4); under C the losses are `shape_mismatch 18, adapter_l3_fallback_sites 18` (P4c, P14) |
| Discovery: A8 restated, break 8 still fails | **feasible** with a hand-stated `None` (§5.5) | P13: no natural unmapped row exists after C |
| String ids a projection | **feasible, scoped** (11B-3): producers give per-row outputs, a projection makes the per-id inputs, readers keep their string keys | 208 `edges_by_from` lines in 40 files (P9) are not touched |
| `ExecRelation` carrier split (M1, M2) | **feasible** | `exec_relation.rs:17-29` reserves it; `.typed_edges` field reads outside the assembly: 56 lines, 10 files (P9) |
| Per-row producers at every rule-bearing merge point | **feasible**: the merge points in eight classes (§3.2); with the cone and solve per id (R1-05) the rule-bearing ones are the computations' INPUTS and the lookups | P6, trace (§3.2) |
| Per-row cones (epic §5.3 "per-occurrence in the engine" read as far as the cone) | **not in 11b**: undefined inside an SCC and not needed by any reader (R1-05); deferred to step 6 (§12) | `capability_cone.rs:2211` (cone per SCC of string ids) |
| The collided-id rule, fixture flipped | **feasible** | `collision_rules_hand_stated` (`detector_context.rs:2393`) pins today's rules; T1 is rebuilt on a fixture whose arms can tell the rules apart (R1-06) |
| Byte-identical (epic row) | **feasible for stages A and B everywhere; stage C everywhere except 14 cross-app CDO results, in SARIF and in JSON** (11B-1) | the fail-closed identity run on the prototype (§3.4): every fixture comparison identical, single-app CDO identical; P12 for B |
| Id-pattern counts per golden dir unchanged | **feasible** | baseline 43 rows / 21,435 occurrences, equal to 10+11a's (P15) |
| CDO collision delta reported | **feasible** | base line (P11); C line (P4c) |
| Non-speed (Decision 58) | **yes** | no timed sub-phase is deleted; 11b is on no peak list (epic §5.12) |
| The union rule moves CDO output | **not feasible to show**: no collided CDO or corpus row differs in facts or coverage (P12) | F10: hand-stated fixtures are the only evidence |

## 1. Goal

There are three stages, and each is closed by its own identity run (§8):

1. **Stage C, the corrections.** J2, J5 and the adapter's caller check read each row's true
   node (`RowNodes.node_of`) instead of the compatibility answers 11a recorded. The alias goes
   away. The `#if` arm whose declaration the surface did not keep gets its own edges. J5 maps by
   node. `decl_anchor` is deleted.
2. **Stage A, the re-keying.** `RoutineIx` becomes a stored type where the producers need it
   (`CallEdge.from_ix`, the per-row producer outputs). `ExecRelation` carries the Primary and
   Cone payloads as separate fields, so `primary()` can no longer reach the typed edges. Each
   rule-bearing producer emits per row, and one projection turns those outputs into the per-id
   inputs the cone, the solve, the event graph and the lookups take today. In this stage the
   projection reproduces each point's current rule exactly, so the stage is byte-neutral.
3. **Stage B, the one rule.** The projection becomes the epic's one deterministic merge
   (Decision 6). Set-valued inputs of a collided id take the full-value union, independent of
   order. Single-valued lookups take the LAST owning row. The duplications listed in §0.2 end.
   The cone and the solve stay per string id; they are only fed differently.

Why: epic §5.3. Today two rows of one id are merged by whatever rule the store they meet in
happens to use. That gives a collided routine's summary one arm's direct facts and both arms'
callees, a combination no build of the code has (P6, R2a + R1). It also pairs one arm's call
edge with the other arm's bindings (R9 first, R5 last). And three of the engine's answers are
known to be wrong (10+11a §4.7-§4.9).

## 2. Non-goals

- **No id format change.** `compute_routine_id`, `stable_routine_id`, `{id}/cs{n}` and
  `{id}/op{n}` keep their bytes. A collided id stays collided as a string.
- **No change to resolution.** The program graph, `RoutineNodeId` and the histogram stay as
  they are (SHA equal at every stage, both universes).
- **The XmlPort table-element discovery is not fixed here.** It covers the 18 `shape_mismatch`
  sites and has its own issue (F3).
- **`Role` stays in step 8.** `dep_routine_ids` (69 sites) keeps its string key. Assembly
  asserts that the rows of every collision group have one role (all primary or all of one
  dependency app), so a per-id role is never a merge (Q5, §4.3).
- **Detector readers are not re-keyed** (11B-3). Only producers and merge points are.
- **No per-row cone, no per-row solve** (R1-05): both stay per string id, fed by the
  projection. Deferred to step 6 with the `pd` arena (§12).
- **Guard frames keep today's semantics** (per id, LAST row; R2d and the span `frames`). The
  known defect is a step-6 discovery (§12).
- **No twin-node fan-out** (Q1): an edge into a node with rows of several ids goes to ONE row,
  as today. Deferred to step 6 (§12).
- **The L4 solve stays one node per string id** (R7). It is fed the projection. Its interner's
  type is renamed `SolveIx` (11A-4).
- **The `pd` arena belongs to step 6** (G-4). That step will key it by this step's `RoutineIx`.
- **No bench** (Decision 58).

## 3. Background

### 3.1 What 11a left

- `RowNodes` (`workspace.rs:2008-2024`) is built in `RowNodes::build` (`:2144-2275`) and dropped
  before post-attach (`program_calls.rs:562`). It holds `node_of` per row, `decl_anchor` per
  caller node of an edge that is read, `j2_row` = J2 (`:2212-2225`), and `j5_node` = J5 for
  event-subscriber rows (`:2232-2267`).
- The caller check compares `decl_anchor[from]` with the row's anchor
  (`program_calls.rs:1189-1197`). `Converter::l3_routine` is `j2_row.get(id)` (`:1464-1469`).
  `program_event_graph` reads `row_nodes.j5(ri)`, and the first MAPPED row per id emits
  (`events.rs:361-376`).
- `collision_census` (`workspace.rs:2078-2123`) prints one `[retained-census] collision` line.
- `RoutineIx` exists only as a position (11A-2). The name is still taken by the L4 interner
  (`src/engine/l4/routine_interner.rs:27`).

### 3.2 The merge points (P6 `p6-inventory.md` + the round-1 trace `p6b-trace.md`)

P6 found 22 points. Round 1 traced every path P6 had left open (`d1_*`, root classification,
`gate/*` including policy, `ordering_inter.rs`, `unresolved_cone.rs`, `dep_artifact_l4.rs`,
`r3a4_projection.rs`, every detector that builds its own id-keyed map) and found about 30 more,
listed with file:line in `/work/bench/160-11b/p6b-trace.md`. All of them sit in eight classes.
The class decides what A keys per row, and what B's projection does. Paths are under `src/`.

| Class | Points | Today | A (producer) | B's projection |
|---|---|---|---|---|
| **U** already a union, or order-free | R1 `edges_by_from` / `edges_from_order` (`engine/l4/combined_graph.rs:418-428`); #10 `uncertainty_edges_by_from` (`engine/l5/detector_context.rs:1597-1605`, `engine/l4/summary_runner.rs:1427-1431`); #12 entry points (`engine/l5/entry_points.rs:63-115`); R3b `uncertainties_by_node`, `parameter_roles_by_routine` (`detector_context.rs:1669-1696`) | union / id-pure; R1 and #10 are stable-sorted by sort key only, so equal-key items keep EMISSION order | unchanged (they already take every row) | single-row ids: unchanged. A collided id's list (only where two or more rows contribute) is sorted by the sort key, then by the full value (`Ord`, §4.6), instead of keeping emission order on a tie (R1-01; T2u) |
| **S** set-valued INPUTS of a per-id computation, merged by a non-union rule | R2a `det_cone_inputs` (`detector_context.rs:1252-1261`); R2b cross-app base `direct_coverage` / `direct_full` / `leaf_summaries` (`engine/l4/capability_cone.rs:3936-3961`); R2c r3a3 builders (`:4368-4397`, `:4565-4577`, `:4726-4735`); #13 solve input `base_summaries` (`summary_runner.rs:1383-1393`: its `db_effects`, `uncertainties` and the two flags; its `parameter_roles` are class P); R4 event `processed` (`program/model/events.rs:366-376`); R4b one `EventSymbol` per publisher row (`:313-320`); R1c interface candidates (`combined_graph.rs:573-672`); #14 `commits_by_routine` (`engine/l5/transaction_spans.rs:487-507`, primary rows only); #16 checked-run seeds (`:598-625`); #17 `prove_event_param_temps` (`program/model/event_param_temp.rs:59-130`) | last / first / per-row duplicate | per-row output | full-value union across the id's rows (§4.6); #17: the all-raises rule (§4.6) |
| **K** per-row duplicates a reader sees k times | k-fold `CombinedGraph.nodes` (`combined_graph.rs:415-416`) → `engine/l5/reverse_call_graph.rs:37-41`, `pending_writes.rs:43-50`, `project_r3a1` `combined_edges` (`combined_graph.rs:1111-1115`); snapshot per-row reads (`engine/l5/snapshot.rs:1353-1385`, `:1862-1921` incl. `derive_callsite_evidence` `:1893-1917`, `:2311-2338`); `classify_roots` (`engine/root_classification.rs:298-319`); `build_dep_order_index` (`engine/deps/dep_artifact_l4.rs:557-619`); policy rule evaluation per row (`engine/gate/policy/policy_engine.rs:195-296`: counters double, two findings share an id); d1 seeds (`engine/l5/d1_graph.rs:201-253`) | k copies, equal or not | unchanged | full-value dedup across the id's rows: equal copies collapse to one, unequal ones stay, sorted by full value |
| **P** single-valued lookups | R1b (`combined_graph.rs:283-331`), R1d (`:541-550`), R5 `upgraded_bindings` (`program/model/program_calls.rs:1352-1381`), R6 `routine_by_id` (`:1336`), R9 `first_resolved_edge_per_callsite` (`detector_context.rs:2028-2037`), #11 `access_modifiers` (`:1527-1535`), #13 `routines_by_id`, `body_avail_by_id`, `stable_map` (`summary_runner.rs:1349-1402`), #19 d46/d48/d61 maps, #20 snapshot anchors (`snapshot.rs:1405-1422`, `:1777-1986`, `:2349`, `:2437`) and `callee_display` / `result_consumed` / `under_asserterror` (`:1977-1985`), snapshot identities (`:1294`), #21 the lookup `collect`s (P6's 14, plus `engine/gate/projection.rs:58`, `gate/format_html.rs:159`, `gate/app_attribution.rs:42`, `engine/l5/fingerprint.rs:96`), #22 `OperationJoin` (`engine/l4/effect_query_cli.rs:462`), `root_classifications_by_routine` (`detector_context.rs:1744-1751`), `ast_by_routine` and `resolve_target` (`root_classification.rs:521-536`: two rows of one id read as "ambiguous"), `root_kinds_by_routine_id` (`policy_engine.rs:151`), `compute_return_summaries` (`engine/return_summary.rs:466-470`), `compute_ordering_facts` (`engine/l5/ordering_facts.rs:218-224`), `scope_frames_by_routine` + `collect_dep_order_index` (`dep_artifact_l4.rs:335-337`, `:986-993`: today a row's ops are paired with ANOTHER row's frames), `evidence_by_id` (`:379-431`), `unresolved_cone.rs:94-96`; **per parameter** (R2-01): the base summary's `parameter_roles` and `dispatch_by_id` (`summary_runner.rs:1362-1369`), keyed `(id, parameter index)`, read by taking the first record per index (`d37.rs:240-245`, `summary_runner.rs:846-860`) and one `ParamDispatch` per parameter; **per-site readers that bypass R9** (R1-04): `d37.rs:224-239` and `compose_roles_only` (`summary_runner.rs:790-860`) find "the first edge with this call-site id" in the unioned `edges_by_from[id]` and read `upgraded_bindings[cs.id]` (R5, LAST) | mostly LW, some FW, some mixed | per `(RoutineIx, key)` where the key is a call site, an op or a parameter index | the LAST OWNING row per key, every piece of one key from that one row (§4.6) |
| **G** guard frames | R2d `guard_frames` (`capability_cone.rs:2383-2394`); #15 span `frames` (`transaction_spans.rs:148, :380`) | LW / FW per id | unchanged | **unchanged** (decision 5; defect filed for step 6, §12) |
| **F** fail-closed | #18 `closed_world_temp` `dup_ids` (`engine/l5/closed_world_temp.rs:105-110, :198, :254`) | a duplicated id is never proven | unchanged | unchanged |
| **R** reader-side finding choice over rows (11B-3: readers are not re-keyed) | `d1_reach.rs:836-860` groups, `d1_dataflow.rs:3164-3186` / `:3308-3322`, `d1_cohort.rs:575-590`; `detectors/d2.rs:253-261`, `:302-533`; `d3.rs:161-624`; `d13.rs:41-94`; `d18.rs:58-129`; `d47.rs:384-452`, `d49.rs:128-150`, `d51.rs:172-204`; `engine/gate/run.rs:565, :576, :617` `kept_ids` | FW / any | unchanged | **unchanged**: they read projected inputs and keep their own first-row choice; listed in the step-6/reader discovery (§12) |
| kept | R7 the solver interner (`engine/l4/routine_interner.rs:96-108`); R8 `ConeDerivedStore` (one per SCC member: Tarjan skips repeated ids, `scc.rs:69-70`); R3a summaries drain (`detector_context.rs:1385-1496`): with the cone per id it only files one per-id result | — | `SolveIx` rename / unchanged | fed by the projection |

**F6 additions (implementation and final panel round 1):** `engine/l5/pending_writes.rs`
`effects` (class P: a collided seed row counts only the sited direct facts it makes itself;
its id's facts are the union); the snapshot's call-site resolutions (`derive_callsite_resolutions`)
and operation index with its frame table (`derive_operation_evidence`,
`derive_routine_order_frames`: class P per key, the frame table holding every row's frames,
shifted per row, so a K copy's `order` and a P op's `order` both name their own row's frames);
the dependency order index's return summaries (class P per id); the checked-run seeds (#16)
are the union of the rows that produce one, each seeded from the last such row. Conductor
decisions on the round-1 spec gaps:
- **Policy evaluation is class P, not K** (FP-03): one evaluation per collided id, through its
  LAST row (`policy_engine.rs` `run_policy_engine`). Policy findings are keyed by routine id,
  and a class K dedup of per-row findings, which carry their row's anchor, cannot collapse
  natural `#if` arms. LIMIT: an earlier arm's distinct facts produce no policy finding.
- **d37 compares a row's `Validate` only with that row's own call sites** (FP-14), the same
  own-facts principle as `pending_writes`: another arm's sites sit at other source positions,
  so "after the `Validate`" means nothing across arms. Each site's callee and bindings still
  come from `site_pick`.
- **LIMIT: order and position comparisons across arms** (FP-12): a collided id's merged
  dependency order entry (and the snapshot's operation index) holds keys of several rows,
  each with its own row's `order_id`; comparing two entries owned by different arms is not
  meaningful. Not changed.
- **LIMIT: readers of the serialized call-site evidence pick by value** (FP-13): the
  snapshot's `callsiteIndex` is class K (each row's copy, sorted by full value) and carries
  no row, so its class P readers (`unresolved_cone.rs`, `digest.rs`, `ordering_engine.rs`)
  pick a collided site's copy by value, not by its LAST owning row. Not changed.
- Neither limit has a CDO witness: CDO's 2 collision groups have equal facts at every point
  (the census and the identity runs). Discoveries are filed for both (carry the owning row or
  anchor in the evidence; per-arm order comparison).

Downstream only (no rule of their own): `d1_graph.rs:134`, `:261`, `:293`; `ordering_inter.rs`
(keyed over the snapshot's already-grouped `callsite_resolutions`); `unresolved_cone.rs:118`,
`:131`, `:184`; `gate/diff/*`; `baseline.rs:94`; the event-id keyed detector maps; d8/d50 span
maps. `d1.rs:1993` and `r3a4_projection.rs:71-74` hold identical values (harmless).

### 3.3 Population (P2, P10, P11, P12)

| Model | rows | ids | groups | aliases | J5 gaps | `caller_mismatch` |
|---|--:|--:|--:|--:|--:|--:|
| CDO cross-app | 31,658 | 31,656 | 2 (one node each; dependency source; Base App `PageManagement`) | 1 | 0 | 5 |
| CDO single-app | 5,479 | 5,479 | 0 | 0 | 0 | 0 |
| fixtures, 246 dirs × 2 universes | — | — | 1 (`ws-cross-object-chain` cross-app: two symbol-only `OnRun` rows) | 0 | 1 (`lsp-diff-deps` cross-app) | 0 |

The two CDO groups are `GetSalesHeaderPageID` (twin rows at one anchor, 5 and 4 call sites) and
`GetPurchaseHeaderPageID` (arms at source lines 325 and 354; the first is `#if not CLEAN26` and
calls `OnAfterGetPurchaseHeaderPageID`, the `#else` arm does not). Every collided row has 0
direct facts. The CDO rows are `complete`; the corpus rows are `unknown` / `opaque-dependency`
(P12).

### 3.4 R-MOVES: what stage C moves (measured, P3-P5, P4c)

| Correction | Identity | Output move |
|---|---|---|
| J2 alias | the 2 routes from dependency row `GetAccessToken` (`…6b4aafd9`, `CoreSessionManager.Codeunit.al` cs4 line 76, cs6 line 85) into node `acquiretokenfromcache`/5 `sig_fp` 8102086367325882817: `to` changes from row `…4f4ac631` to that node's own row `…b353d8a0` | **none in SARIF**: no primary routine reaches row `…6b4aafd9` (P5). J2 sends every call into `getaccesstoken`/0 to its first row `…0eac6119` |
| Caller check | `GetPurchaseHeaderPageID` first arm, 5 sites (source lines 330, 331, 332 ×2, 351): 5 `Unknown(NoProgramSite)` edges become the program's edges (`RecRef.SetTable`, `IsNullGuid`, `Evaluate`, `GetFilter`, `OnAfterGetPurchaseHeaderPageID`) | **14 `d9-transaction-span-summary` results per cross-app scope, each "publishes N event(s)" +1**: the spans whose cone reaches `PageManagement` (958 primary reachers, P5) now count the `OnAfterGetPurchaseHeaderPageID` event that the not-`CLEAN26` arm publishes. Nothing else changes |
| J5 | no CDO row (`j5_compat_rows=0`); `lsp-diff-deps` cross-app: 1 row | none (P4c: every fixture's SARIF identical) |

The 14 results, keyed by span root (from the measured diff): `RunExportEDocReport`,
`RunExportEDocCodeunit`, `OpenDOMail`, `PrintDocuments`, `Run`, `PostAndHandle` (×3),
`PrintOrEMailAll`, `SendPeriodStatements`, `CreateOrSendAutStatements` (×2), `CalcSalesDisc`,
`LogUsage`. Measured on the prototype in `probe-final.patch`, which used exactly §4.2's rules.

**Re-measured fail-closed in round 1 (R1-11, R1-21)** with `identity-run.sh` and `idcheck.py`
(§8), base binary against the prototype: 3,952 analyzer runs (16 CDO, 246 fixture dirs × 2
universes × 2 scopes × SARIF and JSON, each side), every one exit 0 and valid, 1,976 exhaustive
diff reports. `accept` against the round-1 count expectation (`expect-C.json`, superseded by
`expect-C-exact.json` below): 0 violations. The ONLY differences are the 14
results above, in each of cross-app `--scope primary` and `--scope all`, in SARIF AND in JSON
(the `rootCause` text), each message-only with the event count +1; every other report is
identical (JSON apart from its `generatedAt` timestamp); the histogram is identical. The same
reports checked against "everything identical" fail on exactly those 4 CDO reports (the script's
negative check). Evidence: `/work/bench/160-11b/idrun-C/`, `idrun-C.log`, `idrun-C-summary.json`,
`idrun-C-negcheck.json`.

**Pinned by identity in round 2 (R1-11).** `idcheck.py` now records each differing result's
IDENTITY: rule id, fingerprint (`fingerprints["al-sem/v1"]` in SARIF, `fingerprint` in JSON)
and location (file, line, column). The 1,976 reports were regenerated from the saved outputs of
that run (`idrun-C2/`; no new analyzer run). `idcheck.py pin` wrote
`/work/bench/160-11b/expect-C-exact.json`: per cross-app CDO report, the exact set of 14 moved
identities. That is R-MOVES, frozen. Each of the 4 reports holds 34 `d9` results, so 20 `d9`
results per report must NOT move. `accept idrun-C2 expect-C-exact.json`: 1,976 reports, 0
violations. **Negative substitution** (`neg-subst/`): a right-hand SARIF made from the left one
with 13 of the 14 pinned moves applied and the 14th "+1" put on a DIFFERENT `d9` result (another
span root's summary) instead. `accept` fails, exit 1, with exactly two violations: "unexpected
moved result" (the substituted result) and "expected moved result did not move" (the missing
pinned one).

### 3.5 What stages A and B are predicted to move

Nothing on CDO, in either universe, and nothing in any golden:
- A reproduces every point's current rule (§4.5), and the per-id computations are unchanged.
- B changes rules only for collided ids, and on CDO those are the 4 dependency rows of
  `PageManagement`. Their direct facts and coverage are equal (P12) and their edges are already
  unioned (class U). They are in no Primary payload's `from`, and the primary-only producers
  (#14 spans, policy, root classification of primary routines, the snapshot's primary
  inventory) never see them. What remains are class P picks and class K copies over those 4
  rows. Their shared keys (`cs0`-`cs3`) call the same builtins in both arms, and `cs4` has one
  owner. The dependency order index and dependency evidence (`dep_artifact_l4.rs`) may hold
  them; whether any SARIF or JSON byte depends on that index for these rows is not shown by a
  probe. The corpus group (two bodiless rows, P12) is the same.
B's real move is on hand-stated fixtures (§5). If the B identity run moves anything, F5
decides.

## 4. Design

Decisions: **D1** one claim, three stages C → A → B, C = Task 1 (§4.1); **D2** the corrections
read `node_of` (§4.2); **D3** `RoutineIx` stored where producers need it, `SolveIx` (§4.3);
**D4** carrier split (§4.4); **D5** per-row producers, one projection BEFORE the per-id
computations, compat rules in A (§4.5); **D6** the one rule: full-value union across a collided
id's rows for set-valued inputs, the LAST owning row for single-valued lookups (§4.6); **D7**
keep the event graph's `unmapped` branch as a fail-closed guard (§4.2, §5.5).

### 4.1 One claim, three stages, each with its own output (D1)

Order inside the claim: **C, then A, then B.** Each stage is one or more commits, green on
`scripts/ci-steps task`, and each is closed by its fail-closed identity run (§8) before the next
one starts.
- **C first, as Task 1**: it is small (three reads), it runs on today's string-keyed engine, and
  it is the only stage whose output moves. That move (§3.4) is measured on its own, against the
  base. C is committed and identity-checked on its own and is mergeable alone (F4, F5 stop at it).
- **A next**: the mechanical diff, held to byte identity against C's head.
- **B last**: the rule change, measured against A's head. It is predicted byte-identical on CDO
  (§3.5).

### 4.2 Stage C: the corrections read `node_of` (D2)

- **J2 → node-filtered J2.** `j2_row[n]` = the first non-symbol-only row at `n`'s surface
  anchor **whose `node_of` is `n`**; absent if there is none. This CHANGES the anchor map:
  today `first_at` (`workspace.rs:2183-2185`) keeps only the first row per anchor
  (`entry().or_insert`); C makes it keep every row at the anchor in row order, and the J2 loop
  (`:2212-2225`) picks the first with the node. This equals today's J2 on every node that is not
  an alias, so the change is exactly the alias set (P3: 2 routes). It is not "the first / last
  row of the node": on the `GetAccessToken` shape (one node, two ids at one anchor) "last" would
  move `CallEdge.to` on all 10 routes P10 lists.
  **An absent answer** (an alias node with no own row at its anchor) gives what any absent J2
  gives today (`program_calls.rs:1470-1479`): `Target::MissingFromModel` for a primary node (the
  site becomes an honest unknown) and `Target::Outside` for a dependency node (an external
  edge). That is the intended fail-closed behaviour; on CDO and the corpus no such node exists
  (P3: both alias nodes have their own row at the anchor).
- **The caller check → node equality.** `row_nodes.node_of[ri].as_ref() != Some(&ce.edge.from)`
  means `caller_mismatch`. The key join has already paired one expression on both sides, and an
  expression belongs to exactly one declaration, whose node is `node_of`. P4: this disagrees
  with the anchor check on exactly the 5 R-RESIDUAL sites. A row with `node_of == None` now
  turns every paired site into `caller_mismatch` (the anchor check could pass one); P4 counts 0
  such pairs on CDO and every fixture (R1-20). `decl_anchor` and its build loop
  (`workspace.rs:2193-2210`) are deleted, and with them `RowNodes::build`'s `edges` and `demand`
  parameters, which only that loop reads (R1-22). `join_reads` itself stays: the key join reads
  it (`program_calls.rs:1071`).
- **J5 → `node_of`.** `j5_node` is deleted, and with it J5's span map (`workspace.rs:2226-2252`).
  `census::Key` / `anchor_key` stay: the physical census tool still reads them
  (`census.rs:134, :160, :234`; R1-22 partly refuted). `program_event_graph` reads
  `node_of[ri]` for subscriber rows. The `processed` rule (R4) is unchanged in C. **D7:** the `None` arm
  (`events.rs:370-373`) stays as a fail-closed guard: no edges, counted as `unmapped`. P13 found
  no natural `None` after C, but a dependency row of an app the graph does not know has no node
  (`append_dependency_rows`; 10+11a FP-03), so the guard is not dead by construction.
- **The census.** In C, `j2_alias_nodes` and `j5_compat_rows` are computed against the
  corrected answers and must read 0 on CDO (§8). They are deleted in A, together with the
  fields they compare.
- **The CDO loss gate.** `adapter_cross_app_losses_on_cdo` (`tests/program_resolve_harness.rs:10707`)
  asserts R-RESIDUAL minus the 5 `caller_mismatch` sites: 18 `shape_mismatch` and 18
  `adapter_l3_fallback_sites` (P4c), or whatever the XmlPort fix leaves (F3).

### 4.3 Stage A: `RoutineIx`, stored where the producers need it (D3)

- `pub struct RoutineIx(pub u32)` goes in `src/program/model/` and derives `Copy`, `Ord` and
  `Hash`. Its value is the row's position (the same meaning as 11A-2). A row does not store it
  (11B-8). It is stored in two places only:
  - **`CallEdge.from_ix`**, set by the adapter beside `from` (`CallEdge::base`,
    `program_calls.rs:999, :1638, :2155, :2259`). It is not serialized: `CallEdge` is not
    `Serialize` (`program/model/calls.rs:37`). It is what lets the projection tell which row
    emitted an edge, a binding list, or a raise (#17).
  - **the per-row producer outputs** of §4.5, indexed by `RoutineIx`.
  `CombinedEdge` and `TypedEdge` do NOT gain it (R1-16): the combined graph, the cone and the
  solve stay per string id, so no reader of theirs needs a row. Event-dispatch edges, whose
  `from` is a publisher id (`combined_graph.rs:387-406`), therefore need no row either.
- **`rows_of_id` is transient** (R1-12): the projection builds `HashMap<&str, SmallVec<[RoutineIx; 1]>>`
  from the CURRENT `ws.routines` each time it runs, and drops it after. A hand-stated collision
  made after assembly (`force_shared_id`, `detector_context.rs:2051`) is therefore seen. Nothing
  retains it, which also keeps `batch` (two models in one process) safe without a model tag (Q3).
- **Role homogeneity** (Q5): the projection checks that the rows of each collision group share
  one role (all primary, or all of one dependency app, by `source_unit_id` / `app_guid`). The
  check runs INSIDE the production projection, every time it runs. In a debug build it is a
  `debug_assert!` with the message "collision group mixes roles"; in every build it increments
  a census counter, `groups_mixed_role`, which must read 0 (P11's line gains it). Test T9 drives
  it through the production path (R2-02).
- The L4 interner's type is renamed `SolveIx` (11A-4). This is mechanical, in 12 files (P9).
  `alsem query` output does not change (`cli-query-goldens`).

### 4.4 Stage A: the carrier split (M1, M2) (D4)

```rust
pub struct ExecRelation {
    primary: Arc<PrimaryGraph>,   // edges_by_from / edges_from_order / uncertainty_edges /
                                  //   nodes: today's field names, Primary payload only
    typed: Vec<TypedEdge>,        // Cone payload, today's concatenation (Decision 43)
    scc: Arc<SccResult>,
    dependency_uncertainty: Vec<UncertaintyEdge>,
}
```

`ExecRelation::new` splits the assembled `CombinedGraph`: `typed` is moved out, not copied.
`primary()` returns `&Arc<PrimaryGraph>`, which has no typed edges, so the M2 guarantee now holds
in both directions. Every `graph.typed_edges` reader that is fed by a relation moves to
`rel.cone()` (d43, the snapshot and digest paths; the plan lists them from P9's 56 lines).
Field names stay, so the 208 `edges_by_from` lines change only the type name they import.

### 4.5 Stage A: per-row producers, one projection BEFORE the computation (D5)

The cone, the solve, the event graph and the transaction spans stay computed per string id,
exactly as today (R1-05; decision 2). A changes only what feeds them:
- **Per-row producer output.** Each class S, K and P point's producer emits per row (a
  `Vec<_>` indexed by `RoutineIx`, or keyed `(RoutineIx, key)` for a call-site or op key):
  `direct_facts_for_routine`'s three outputs, the base summary and dispatch bindings, the
  publisher symbols, the interface candidates, the commits and checked-run seeds, the raises
  (`CallEdge.from_ix`), the call-site edge and bindings, and each class P lookup's value.
  Where the producer is already a `for r in &ws.routines` loop, this changes only what it
  collects into.
- **One projection, before the computation.** `program/model/projection.rs` (new) turns the
  per-row outputs into the per-id inputs the computation takes today. It uses the transient
  `rows_of_id` (§4.3). An id with one row passes its value through, moved and untouched (no
  dedup, no re-sort, no copy; R1-01, R1-09). Only collided ids are merged.
- **In stage A the projection reproduces each point's current rule, before the computation**
  (sol #5): LAST for R2a/R2b/R2c, #13 and most class P; FIRST for R1b, R9, #22 and
  `collect_dep_order_index`; per-row duplicates kept for class K and R4b; the current
  `processed` rule for R4; the current LAST-row raise inspection for #17; the current pairing
  for `scope_frames_by_routine` / `collect_dep_order_index` (frames from the last row, ops from
  the first). Because the computation then runs on the same per-id inputs as today, its output
  is today's. There is one function per rule, and each has a doc line naming the point(s) it
  serves. Stage A's identity run (§8) is the proof, and T1-T1g pass unchanged through A (§5).
- **`AbiRowIds` stays node → string id** (R1-15). `Converter::routine_by_id` (R6) becomes the
  projection's class P pick, which in A and in B is the LAST row of the id, i.e. today's
  `collect` answer. So nothing about symbol-only targets moves in either stage.
- **Readers are not re-keyed** (11B-3). A detector that reads `edges_by_from[id]` or
  `ctx.routine_by_id[id]` reads the projected value. Class R points keep their own choice.

### 4.6 Stage B: the one rule (Decision 6) (D6)

Stage B replaces the compat functions with two rules, applied ONLY to collided ids (an id with
one row is passed through untouched, R1-01):
- **Set-valued (classes S, K): the full-value union across the id's rows.**
  - **Identity is the whole payload** (decision 3): two items are the same item only if every
    field is equal. Equal items collapse to one; items that share a sort key but differ in any
    field (an edge's `resolution`, `dispatch`, `subscriber_app_id`; an uncertainty's
    `interface_name`; a publisher's `parameters`, `signature_hash`, `isolated`) are all kept.
  - **Order**: today's sort key first (`edge_sort_key`, `uncertainty_sort_key`,
    `inherited_fact_key`, `combined_graph.rs:175-196`), then the full value as the tie-break,
    by a TOTAL ORDER on the payload type, never a serialization (R2-04: `serde_json` writes
    struct fields in declaration order and maps in iteration order, and this crate's
    `serde_json` pulls in `indexmap`, so no serialized form is canonical; `Debug` has the same
    problem). Each payload type in a union derives `PartialOrd, Ord` (or, where a field has no
    `Ord`, implements an explicit comparator over every field). Deriving `Ord` is itself the
    check that no field is hash-ordered: `HashMap`/`HashSet` and `f32`/`f64` have no `Ord`, so
    the derive does not compile. Task A asserts it per type, in a `--lib` test that names every
    union payload type and calls a `fn total<T: Ord>()` on it. Read at this base, none of the
    payload types has such a field: `CombinedEdge`, `TypedEdge`, `UncertaintyEdge`
    (`combined_graph.rs:37, :64, :83`), `Uncertainty`, `DbEffect` (`engine/l4/summary.rs:68,
    :87`), `CapabilityFact` (`program/body/capability/mod.rs:169`), `RootClassification`
    (`engine/root_classification.rs:96`), `SnapshotCallsiteEvidence` (`snapshot.rs:749`);
    `EventSymbol` and `EventEdge` (`program/model/events.rs:45, :84`) derive only `Debug, Clone`
    today and gain `PartialEq, Eq, PartialOrd, Ord`. A nested type that cannot derive `Ord`
    fails the build, which is the point. So the result does not depend on row order (T2, T2u).
  - **Scalars**: flags OR-ed. Coverage status: the least complete (direct `complete` <
    `partial` < `unknown`; inherited `complete` < `partial`; P8, decision 7). Reasons: a sorted
    union.
  - **Per point**: R2a/R2b/R2c direct facts, coverage and leaf summaries; #13 base summary
    (`db_effects` and `uncertainties` as full-value unions; the two flags OR-ed; its
    `parameter_roles` are NOT unioned, see class P below); class U's collided lists (R1, #10:
    the tie order above, decision 1); R4b publisher symbols; R4: each subscriber id emits the
    full-value union of the subscriptions of its rows' distinct nodes (equal to today when an
    id's rows share a node, every CDO and corpus group); R1c interface candidates (one per
    distinct implementer, so `candidate_count` is no longer doubled); #14 commits and #16
    checked-run seeds (one span per call-site id); class K copies (the reverse graph and r3a1
    list each id's edges once; the snapshot, root classifications, the dependency order index,
    the policy evaluation and d1 seeds keep one copy of each distinct item).
  - **Publisher facts in direct facts** (R1-17): `publisher_events_by_routine` is keyed by the
    publisher's string id (`detector_context.rs:1243-1252`), so each row of a publisher group
    sees every row's events. Under the union that is harmless (equal facts collapse). The tests
    therefore assert no per-row publisher facts.
- **#17 `prove_event_param_temps` (decision 6; R1-03).** Each raise is read at its own
  `(RoutineIx, site)` through `CallEdge.from_ix`, not through a LAST-row string index
  (`event_param_temp.rs:59-130`). A subscriber parameter is proven temporary only if EVERY
  applicable raise, over every row of every caller id, proves temporary. When it is proven, the
  rewrite applies to every subscriber row of the id. A collided caller whose arms pass a
  physical record in one arm and a temp record in the other is therefore never proven (T1f-neg).
- **Single-valued (class P): the LAST OWNING row per key (decision 4; R1-04).** For each key,
  the owning rows are the rows of the id that have that key (§ Words). The pick is the last of
  them in `RoutineIx` order. So `{id}/cs4` on the CDO 5/4 shape comes from the 5-site arm even
  though the 4-site arm is last.
  - **Every piece of one key comes from that one row**: a call site's resolved edge (R9) AND
    its bindings (R5) AND its snapshot anchor and display (#20). Today R9 is FW and R5 is LW, so
    a collided site can pair one arm's edge with the other arm's bindings. The same holds for
    `scope_frames_by_routine` + `collect_dep_order_index`: one row's frames with that row's ops.
  - **The within-row candidate policy is kept**: R9 still takes, inside the chosen row's site,
    the first targeted edge (the smallest implementer, `detector_context.rs:2019-2037`).
  - Points that are FW today (R1b, R9, #22, `collect_dep_order_index`) change to it; LW ones
    already agree where the last row owns the key. `resolve_target` (root classification with a
    config) dedups its matches by id first, so two rows of one id no longer read as "ambiguous".
  - **Per parameter, never a union** (decision 3; R2-01). A parameter's role
    (`RecordRoleSummary`) and its dispatch (`ParamDispatch`) are single-valued, keyed
    `(id, parameter index)`: every reader takes ONE record per index (`d37.rs:240-245`,
    `summary_runner.rs:846-860`, the walker's one `ParamDispatch` per parameter), so two kept
    records would mean "the first one", not their combination. The rows of one id share their
    signature (same id ⇒ same parameter list), so every row owns every index and the pick is the
    LAST row. That is today's answer for the base summary (`base_summaries` is LW, #13); the
    change is that the union no longer touches the field. Tested with conflicting roles through a
    caller and through d37 (T1j).
  - **Per-site consumers go through the same pick** (decision 2; R1-04). `d37`
    (`d37.rs:224-239`) and `compose_roles_only` (`summary_runner.rs:790-860`) today iterate a
    row's own call sites, take "the first edge with this call-site id" from the UNIONED
    `edges_by_from[id]` (which may be the other arm's edge), and read `upgraded_bindings[cs.id]`
    (R5, the last row's bindings). At B both read one accessor, `site_pick(id, cs_key)`. It
    returns the LAST owning row's `PCallSite`, its first targeted `CallEdge` (R9's within-row
    policy, through `CallEdge.from_ix`) and its bindings, so edge and bindings always come from
    one row. They iterate the id's owned call-site keys (the union of its rows' keys), not one
    row's list.
    **For a single-row id the answer does not change**: its owning row is its only row, its
    bindings are R5's, and "the first targeted `CallEdge` of that site" equals "the first
    `CombinedEdge` with that call-site id". Both lists hold that site's resolved targets in
    target order: the adapter emits a multi-target (interface) site sorted by `to`, and
    `edges_by_from` is stable-sorted by `kind|callsite|to` (`combined_graph.rs:175-183`), with
    one kind per site. In A, `site_pick` is a COMPAT accessor that reproduces today's mix
    exactly (the row's own `cs`, the first unioned edge, the LAST row's bindings), so A stays
    byte-neutral. B switches it. The single-row equality is then proved two ways: by T6b's
    single-row multi-implementer case (B's answer equals today's), and by B's identity run (every
    CDO and fixture output identical). If either shows a changed single-row answer, the
    exception is stated, and the accessor keeps today's edge choice for single-row ids with a
    test of its own (F11).
- **Class G (guard frames) is unchanged** (decision 5): R2d stays per id with the LAST row's
  frame, and #15 stays as today. Unioning conjunctive requirements of alternative arms would
  describe an impossible path, and LAST can prune another arm's commit. That defect is filed for
  step 6 with the per-row cone (§12). T-guard pins today's behaviour.
- **Class R is unchanged** (11B-3): those detectors read the projected inputs and keep their own
  first-row choice of which finding to emit. Listed in the reader discovery (§12).
- **R7** (the solve's interner) keeps one node per id and is fed the projected inputs.
- **Class F** is unchanged.

### 4.7 Determinism (I7)

`RoutineIx` order is today's row order. The projection walks rows in `RoutineIx` order. The
full-value union is commutative and idempotent and its output is sorted by (sort key, full
value), so set-valued output does not depend on which row comes first (T2). The LAST owning row
depends on row order by design, and row order is fixed by assembly (workspace files, then ABI
rows, then dependency source files, in path order). It DOES show when the order is reversed:
two arms sit at different lines, so a key both own (the routine id, a shared call site) gets a
different anchor from a different row (R2-03; the earlier "the pick cannot show" was wrong).
That is why T2 compares only set-valued surfaces with the collided rows' anchors normalized, and
excludes classes P, R and G by design. T6 pins the pick itself.
`rel.typed` keeps today's concatenation (Decision 43). The Primary SCC never sees duplicates:
Tarjan skips repeated ids (P7), so `rel.scc` and every serialized SCC count are unchanged.

### 4.8 Memory

**Ownership** (R1-09): the per-row producer outputs are intermediates. The projection consumes
them by value: a single-row id's value is MOVED into the per-id input, a collided id's values are
merged into one, and the per-row vector is dropped before the computation runs. Nothing keeps a
per-row copy beside its projection (the §7 sentence that said otherwise is withdrawn). The only
retained additions are `CallEdge.from_ix` and the census counter.

| Item | Lives | Change, CDO cross-app (an estimate; the census reports the real bytes) |
|---|---|---|
| `RowNodes.decl_anchor`, `j5_node` (C) | build-local | deleted; `program.row_nodes` is 8,809,976 B today (P11) |
| `CallEdge.from_ix` (A) | retained with `model.calls` | a `u32` in a `String`-aligned struct: +8 B per edge × 84,717 edges (10+11a P15) ≈ 0.68 MB |
| transient `rows_of_id` (A) | inside the projection | ≈ 31,656 keys (`&str`, 16 B) + `SmallVec<[RoutineIx; 1]>` (16 B) + slot / 0.875 load ≈ 1.5 MB while the projection runs, then freed |
| per-row vectors (A) | until the projection consumes them | one `Vec` header (24 B) per row per producer × 31,658 rows ≈ 0.76 MB per producer, transient; the payloads are moved, not copied |
| carrier split | relation | `typed` moved out of the assembled graph, not copied: 0 |

11b has no peak bound (§5.12 does not list it), and no negligible-overhead claim is made: the
census is read per stage and reported (§8, F7).

### 4.9 What is deleted

- C: `RowNodes.decl_anchor` and `j5_node` and their build code (`workspace.rs:2193-2210`,
  `:2226-2267`); `RowNodes::build`'s `edges` and `demand` parameters (R1-22). Kept:
  `census::Key` / `anchor_key` (read by the physical census, `census.rs:134, :160, :234`) and
  `join_reads` (read by the key join, `program_calls.rs:1071`).
- A: `CollisionCensus.j2_alias_nodes` and `j5_compat_rows`; `Converter::routine_by_id`
  (replaced by the projection's class P pick, same answer, R1-15); the name `RoutineIx` in
  `l4` (renamed `SolveIx`).
- B: the compat functions; the R3a skip comment block (`detector_context.rs:1412-1468`) and the
  `debug_assert` on how cones and `direct_full` collide (I2's exception, §5.9).

## 5. Test plan

Every new or restated test lists its break (made with Edit, reverted with Edit), the failing
assertion and the revert, and both outcomes are recorded (I9). A scripted break asserts that it
applied.

**Fixtures** (R1-06, R1-12): a collision is made the natural way where possible, by one
procedure declared in an `#if` / `#else` pair with the same signature. The lowerer emits two
rows with one id and one node (the `GetPurchaseHeaderPageID` shape). Each test first asserts its
precondition from the CURRENT rows: the group's size (2), its node count, and the property the
assertion depends on (for example, "the arms' direct facts differ"). Where a test needs a shape
the source cannot make (two nodes behind one id, a mixed role), it uses `force_shared_id` by
assignment. That is safe because `rows_of_id` is rebuilt from the current rows (§4.3). When a
test needs the call-site ids to collide, it uses a sibling helper `force_shared_id_with_sites`
that also re-keys `cs.id` / `callsite_id`; `force_shared_id` re-keys only routine ids
(`detector_context.rs:2051-2075`) and stays as it is.

**Every distinct merge operation has a production-use assertion and a mutation** (R1-08):

| Operation | Production use pinned by | Mutation → fails |
|---|---|---|
| compat projection (A) | the A identity run (§8) and every test below passing unchanged through A | swap one compat function's rule (LAST ↔ FIRST) → that point's test fails at A |
| full-value union, S (B) | T1, T1b (R4b, through the production event graph and `rel.primary().edges_by_from`), T1c, T1e, T2 | first-row / last-row pick; dedup by sort key only |
| full-value dedup, K (B) | T1d, T1g, T1h, T1i (production `policy check`) | keep per-row copies |
| union tie order, incl. class U's collided lists (B) | T2u (through production `edges_by_from` / `uncertainty_edges_by_from`) | emission order instead of full value |
| total order per payload type | the `--lib` test of §4.6 (`fn total<T: Ord>()` per union payload type) | add a `HashMap` field to one payload → the build fails |
| LAST owning row, P (B) | T6, T1g | `rows_of_id.last()` without the owning filter; edge and bindings from different rows |
| per-parameter pick, P (B) | T1j (through a caller's summary and d37) | union the role records |
| per-site consumers through `site_pick` (A compat, B consistent) | T6b (d37 and `compose_roles_only`) | read `edges_by_from` directly again |
| within-row candidate policy (kept) | T6 | take the last implementer |
| all-raises proof, #17 (B) | T1f, T1f-neg | inspect by LAST-row string index |
| least-complete coverage (B) | T1-cov | LAST status; LAST reasons |
| guard frames unchanged (G) | T-guard | union the arms' requirements |
| role homogeneity check, in the production projection | T9 | remove the projection's call to the check |

T2 covers set-valued surfaces only; classes P, R and G are excluded from it by design (T6, T6b,
T1g and T1j pin P; R is out of scope by 11B-3; G is unchanged and T-guard pins it). No test
claims to cover every possible merge point: T2 catches a missed SET-VALUED point on the surfaces
it compares, and the per-operation tests above pin the rest.

### 5.1 T1: the collision fixture flips (the epic's proof row; rebuilt for R1-06)

`collision_rules_hand_stated` (`detector_context.rs:2393`) is rebuilt on a natural `#if`
collision whose arms can tell the rules apart. Today's fixture cannot: row A is
`parse_incomplete`, so `direct_facts_for_routine` returns no A facts (`capability_cone.rs:524-537`),
and `TouchA` / `TouchB` have empty bodies. The new fixture is procedure `Work` with two arms.
Arm A does `A.Insert(); TouchA();`, and `TouchA` writes table X. Arm B does `B.Modify(); TouchB();`,
and `TouchB` writes table Z. Both arms are complete. Preconditions: the two rows' direct facts
differ and are both non-empty; `TouchA` and `TouchB` write different tables.
- **Through C and A** (today's rules): the projected direct facts are arm B's (LAST, R2a), the
  summary's inherited half has X and Z (R1 edges are a union), and `edges_by_from` has both
  callees.
- **At B**: the direct facts are A ∪ B (A's insert AND B's modify); the inherited half still has
  X and Z; the summary's `db_effects` hold both arms' direct writes.
- Breaks at B: projection first-row → "B's modify in the direct facts" fails; last-row → "A's
  insert" fails. Revert each. The per-row derived-row assertion of the previous draft is dropped
  (no per-row cone, R1-05).

**T1-cov** (decision 7): the same fixture, with arm A set `parse_incomplete` and arm B set
`body_available = false` by assignment (coverage-only test, so the assignment is the stated
precondition). At B, through the PRODUCTION consumers, the routine's coverage record from
`build_detector_context` and the snapshot's `coverage` entry: status `unknown`, reasons
`["opaque-dependency", "parse-incomplete"]` (the sorted union). A second case has arm A
`partial` (one reason) and arm B `complete`: status `partial`. Breaks: LAST status → the second
case reads `complete`; LAST reasons → one reason missing.

### 5.2 T2: the order swap over the set-valued surfaces (R1-08, R2-03; lands at B)

A natural `#if` collision in a workspace routine. Its arms agree on every key both own: the same
calls at `cs0`-`cs2`, the same operations, the same access modifier and declaration shape. Arm B
alone owns extra keys: a call `cs3` to a routine that writes Z, a commit, and a raise of a
published event. A `#[doc(hidden)]` test entry assembles the model with the rows of every
collision group in REVERSED order (before `RowNodes` and the attach, so every row index stays
consistent). The source is the same in both runs; only the row order differs.

**What is compared** (R2-03): only SET-VALUED surfaces, built by the production code in both
orders:
- the per-id direct facts and coverage the cone takes (`det_cone_inputs`);
- the collided id's projected base summary (`db_effects`, `uncertainties`, the flags);
- `rel.primary().edges_by_from[id]` and `uncertainty_edges_by_from[id]`;
- the reverse call graph;
- the transaction spans (seeds and commits);
- the snapshot's capability facts and call-site evidence for the routine.

**Location normalization**: every source anchor that falls inside either arm's declaration span
is replaced by `(arm, line - arm start, column)`. The arm is named by its SOURCE position (first
or second `#if` arm), which is the same in both runs. The routine's own declaration anchor and
the anchors of the keys both arms own are masked, because a class P pick decides them.

**Excluded by design**: class P lookups (anchors, display, bindings, the routine row), class R
detectors' choice of finding, class G frames. These follow row order on purpose (§4.7) and are
pinned elsewhere (§5.0).

Preconditions:
- the group's size is 2;
- the arms' shared call sites have equal callee text;
- the reversed model really has the arms in the other order;
- the compared surfaces are non-empty and each mentions the collided id (listed by the test, so
  a surface that silently drops the id fails the precondition). The preconditions are
  surface-specific: the event graph is NOT compared here, because this fixture's collided
  routine is an ordinary event caller, which event symbols and subscriptions do not record; the
  event graph's union is pinned by T1b (R3-S1).

The test asserts that each compared surface, normalized, is equal in both orders. Break: revert
any one union to a first-row pick (recorded for R2a, #13 and the snapshot's K dedup) → that
surface differs → fails. Revert. Before B the two orders differ (A's compat rules pick by row),
so the test lands with B. The R4b and policy unions are pinned by their own production witnesses
(T1b, T1i), because this fixture's collided routine is not a publisher and `analyze` does not
run the policy evaluation (R1-08).

**T2u** (R1-01): the projection's union, called on hand-stated items with an equal sort key and
different payloads (two edges `kind|{id}/cs0|to` differing in `resolution`; two uncertainties
differing in `interface_name`; two publisher symbols differing in `signature_hash`), in both
input orders: the outputs are equal, keep BOTH items, and are ordered by full value. Also
through production (decision 1), for class U: two collided rows' call edges with one call-site
id and one target but different `resolution` and `dispatch`, made so by assignment on
`model.calls` (each row's `from_ix` kept), are built into an `ExecRelation` in both row orders.
`rel.primary().edges_by_from[id]` must be equal in both orders, hold both items, and order them
by full value; today's stable sort keeps emission order, so the test fails before B. The
production uncertainty half asserts the class-P rule instead (R3-S2): R1b keeps ONE interface
uncertainty per call-site key, chosen from the LAST owning row (`combined_graph.rs:308-348`
emits once per site), so with two collided rows whose uncertainties at one call-site key differ
in `interface_name`, `uncertainty_edges_by_from[id]` holds exactly the LAST owning row's
uncertainty in both row orders' normalized form; the two-item union of conflicting
uncertainties stays a projection-helper test (above), not a production assertion. Break: dedup by sort key → one item lost
→ fails; insertion order → the two orders differ → fails. A single-row id with an internal
duplicate is passed through untouched (asserted with the same mutation).

### 5.3 T1b-T1j, T-guard, T9: one fixture per remaining class

| Test | Shape | Assertion at B (through C and A: today's behaviour) | Break → fails |
|---|---|---|---|
| T1b `collided_publisher_symbols_merge_by_value` (production witness for R4b) | a publisher declared in two arms, with a subscriber; a variant whose arms differ in a parameter | through the production assembly (`program_event_graph`) and the production relation (`rel.primary().edges_by_from[publisher id]`): equal symbols → one `EventSymbol` and ONE event-dispatch edge per subscriber; differing symbols → both kept, full-value order (no per-row publisher facts are asserted, R1-17) | push a symbol per row → two dispatch edges |
| T1c `collided_interface_call_counts_each_implementer_once` | both arms make one interface call | `candidate_count` = the number of implementers | union without dedup |
| T1d `collided_id_is_one_reverse_entry` | a collided caller | `reverse_call_graph` and r3a1 `combined_edges` list each edge once | walk duplicate `nodes` |
| T1e `collided_commit_seeds_from_both_arms` | a primary routine; one arm commits, the other does not | the span seeds from the union of commits, one span per call-site id, the seed row is a row that owns the commit | seed `routine_by_id[id]` (last row) |
| T1f `event_param_temp_proves_over_every_raise` | both arms of a collided caller pass a temp record | every subscriber row gets the temp rewrite | rewrite the last row only |
| T1f-neg `event_param_temp_mixed_raise_is_not_proven` (decision 6) | arm A passes a PHYSICAL record, arm B (the last row) a temp record, at the same `{id}/csN` | no rewrite on any subscriber row (today: proven, because both raises are read from the last row) | inspect raises through the LAST-row string index |
| T1g `dep_order_index_pairs_frames_and_ops_of_one_row` | a source-bearing dependency with a collided routine whose arms differ in ops | each `{id}/opN` and its scope frame come from that op's last owning row | today's pairing (frames LAST, ops FIRST) |
| T1h `collided_routine_classifies_once` | an entry-point trigger in two arms; a variant with a roots config naming it | one classification per distinct value; `resolve_target` resolves it (not "ambiguous") | keep per-row copies; count rows instead of ids |
| T1i `policy_evaluates_a_collided_routine_once` (production witness for the policy path) | a collided fixture plus a policy file with a rule matching the collided routine; run through the production `alsem policy check` entry (`gate/policy/pipeline.rs:273-277`), not the engine function | the report's `routines_*` counters count the id once; one finding, one finding id | evaluate per row |
| T1j `collided_parameter_role_is_one_record` (R2-01) | two arms of a callee with the same parameters, whose bodies give parameter 1 CONFLICTING roles (one arm writes the record, the other only reads it); a caller passes a `var` record to it; d37's shape on the caller | the callee's projected `parameter_roles` hold ONE record for index 1, the LAST row's; the caller's composed role (through `compose_roles_only`) and d37's verdict both follow that record | union the role records → two records for index 1, and the caller's role follows the first |
| T-guard (decision 5) | arms with opposite guard requirements, and a variant with one unguarded arm | the frame is the LAST row's at A and B (unchanged), and the cone and spans read it as today | union the arms' requirements |
| T9 `collided_rows_share_one_role` (R2-02) | a group forced across two roles by assignment (one row's `source_unit_id` set to a dependency unit), then the PRODUCTION projection run on that model (through `build_detector_context`) | in a debug build the projection's `debug_assert!` fires: `#[cfg_attr(debug_assertions, should_panic(expected = "collision group mixes roles"))]`; in a release build the census counter `groups_mixed_role` reads 1. Natural fixtures and CDO read 0 | remove the projection's call to the check → no panic (debug) and 0 (release) → fails |

### 5.4 T3: J2 restated (10+11a T7a-c)

- `j2_row_one_anchor_two_nodes_is_the_first_row` → `…_is_each_nodes_own_row`: each node maps to
  its own row. Break: drop the node filter → node B maps to row A → fails.
- `j2_row_two_anchors_one_node_is_the_last_arm`: `j2_row` is unchanged (the surface's anchor),
  and the first arm's site is now `matched`, not `caller_mismatch`. Break: restore the anchor
  caller check → `caller_mismatch` → fails.
- `j2_row_one_anchor_one_node_is_the_first_row`: unchanged.

### 5.5 T4, T5: the caller check, J5, A8

- `caller_check_reads_captured_decl_anchor` → `caller_check_reads_node_of`: set the caller row's
  `node_of[ri]` to another node by assignment → its sites are `caller_mismatch`. Break: make the
  check always pass → fails. `caller_anchor_disagreement_is_caller_mismatch`
  (`program_calls.rs:2617`) moves a row's ANCHOR. It is restated to the new contract: a moved
  anchor alone no longer fails the check (the key decides the site, the node decides the
  caller). Its reason is stated in the commit (I2's exception).
- `j5_node_keeps_the_dependency_span_map_answers` (`tests/r4/r4_cross_app_program.rs:1894`) →
  `event_graph_reads_node_of` (R1-07). Today's T8a fixture cannot discriminate: both `Hb` nodes
  subscribe to the same `OnA` (`r4_cross_app_program.rs:1840-1975`), so the old mapping gives
  the same event graph. The fixture changes so the two-node shape's arms subscribe to DIFFERENT
  events of `Dep Pub` (`OnA` in one arm, a new `OnB` in the other; the arms already differ in a
  parameter type, so they stay two nodes). Precondition: the two nodes' subscriptions differ.
  The test asserts that row B's edges are `OnB`'s, row A's are `OnA`'s, and the arm that was
  unmapped now maps. Break: map a row to the first node at its span (the old rule) → row B emits
  `OnA` → fails. Revert.
- **A8** (`tests/r4/r4_cross_app_program.rs:1743`, the ledger discovery): the precondition is
  restated on the corrected mapping (both rows of `H` map to ONE node, both `Some`), and the
  each-edge-once assertion is unchanged. Break 8 ("processed marked before a row maps") is
  vacuous once both rows map (P13), so A8 adds a second case. `cross_app_rows_before_attach`
  cannot serve it: it drops the program, and `program_event_graph` needs its
  `ProgramContext` (`program_calls.rs:882-890`, `events.rs:281`; R1-18). So a new
  `#[doc(hidden)]` entry `cross_app_event_graph_after(inputs, model_instance_id, edit: impl
  FnOnce(&mut ModelRows))` is built on `cross_app_assemble` (`program_calls.rs:827`, which keeps
  the program). It applies `edit` to the rows and returns the event graph. The case sets
  `node_of[first] = None` and asserts that `H` still emits each edge once. Break 8 fails that
  case. Revert. That is D7's guard, pinned.
- `j5_compat_rows_counts_the_subscriber_gaps_only` is deleted with the counter (A).

### 5.6 T6: call-site id collisions (R5, R9)

The CDO 5/4 shape, made naturally (decision 4; R1-04): arm A has 5 call sites, arm B (the last
row) has 4. `cs0` calls a different routine in each arm (different parameters, so different
bindings). `cs1` is an interface call with two workspace implementers in both arms. `cs4` exists
only in arm A. At B:
- `{id}/cs0`: the resolved edge (R9) AND the bindings (R5) are arm B's, and the test asserts
  they come from the SAME row (`from_ix` of the edge = the row whose bindings were taken);
- `{id}/cs4`: edge and bindings are arm A's, the only owner;
- `{id}/cs1`: the edge is the FIRST targeted implementer inside arm B's site (the within-row
  policy, `detector_context.rs:2019-2037`).
Breaks: pick `rows_of_id(id).last()` without the owning filter → `cs4` loses its edge → fails;
edge FIRST row and bindings LAST row (today's mix) → the same-row assertion fails; take the last
implementer → the `cs1` assertion fails. Revert each. Through C and A the test asserts today's
mix (R9 first row, R5 last row), which is A's identity on this input.

**T6b** (decision 2; R1-04): the same 5/4 fixture, read by the two per-site consumers that
bypass R9 today:
- d37's verdict on the caller (`d37.rs:224-245`);
- the caller's composed parameter roles from `compose_roles_only` (`summary_runner.rs:790-860`).

The setup: `cs0`'s two arm targets give the passed `var` record different roles (one writes it,
one only reads it). At B both consumers follow arm B's target AND arm B's bindings (the LAST
owning row), and they visit `cs4` (arm A's only), which today's code skips when the routine row
it is handed is arm B. A single-row multi-implementer case on the same fixture asserts that B's
answer equals today's (§4.6). Breaks:
- read `edges_by_from[id]` directly again → the collided case follows the first arm's edge with
  the last arm's bindings → fails;
- iterate one row's call sites → `cs4` is missing → fails.

Revert each. Through C and A the test asserts today's answers.

### 5.7 T7: `from_order` with a collision group

d17's two-caller fixture (step 4) plus a collided caller: the projected sequence equals today's
(first appearance per id). Break: no dedup → the id appears twice → fails. Revert.

### 5.8 T8: the census after C, and the CDO loss gate

`collision_census_counts_groups` (`program_calls.rs:3197`) is unchanged. C's CDO run prints
`j2_alias_nodes=0 j5_compat_rows=0` (§8), and then the fields go in A.
`adapter_cross_app_losses_on_cdo` asserts the residual minus the 5; its discrimination is
re-recorded (restore the anchor check → the 5 reappear → fails).

### 5.9 Tests deleted or rewritten with the code they pin (I2's exception)

- Rewritten: T3's J2 test; T4's two caller tests; T5's J5 test, its T8a fixture and A8; T1
  (rebuilt fixture) and T1b-T1j, T6, T6b (flipped at B);
  `hand_stated_id_collision_keeps_a_real_summary_and_derived_row` (`detector_context.rs:2302`)
  and `colliding_ids_keep_the_full_summary_union_not_just_the_edges` (`:2811`), which encode R3a
  and are restated to the union at B on the same fixtures. A sibling test helper
  `force_shared_id_with_sites` also re-keys call-site ids; `force_shared_id` and its callers are
  unchanged.
- Deleted: `j5_compat_rows_counts_the_subscriber_gaps_only`.
- Renamed: the interner's tests (`SolveIx`).
- Changed by expected set only: `adapter_cross_app_losses_on_cdo` (C).

Each is named in its commit.

## 6. Acceptance matrix

| # | Item | Evidence |
|---|---|---|
| A11b.1 | J2 alias gone | T3; C's CDO census `j2_alias_nodes=0` |
| A11b.2 | J5 reads `node_of`, with a fail-closed guard | T5, A8 (both cases); C's CDO census `j5_compat_rows=0` |
| A11b.3 | The 5 `caller_mismatch` sites leave the residual | T4; the CDO loss gate = 18 / 18 (or F3's set) |
| A11b.4 | C moves exactly R-MOVES | C's identity run, `accept` against `expect-C-exact.json` with 0 violations: cross-app CDO differs at each scope in exactly the 14 pinned identities (rule + fingerprint + location) of §3.4, in SARIF and JSON, each message-only, event count +1, and no other result of any rule; every other report identical; the 8 golden targets and the histogram SHA identical |
| A11b.5 | `RoutineIx` stored where producers need it; `SolveIx` | §4.3; `CallEdge.from_ix`; `rg -w RoutineIx src/engine/l4` hits only the model's type |
| A11b.6 | Carrier split (M1, M2) | §4.4; `PrimaryGraph` has no `typed_edges` field |
| A11b.7 | Every class S/K/P point produced per row and projected once, before its computation | §4.5; the plan's per-point checklist from §3.2 (classes S, K, P); classes U, G, F, R and kept: unchanged, stated |
| A11b.8 | Stage A byte-neutral | the 8 golden targets and the `--lib` outcomes (apart from §5.9); A's identity run against C's head with `expect-identical.json`: 0 violations |
| A11b.9 | The one rule | T1, T1-cov, T2, T2u, T1b-T1j, T6, T6b, T7, T-guard, T9 and the total-order test (§5.0's operation table, each with its mutation) |
| A11b.10 | Stage B byte-identical outside the hand-stated tests | B's identity run against A's head with `expect-identical.json`: 0 violations, or F5's bounded admission |
| A11b.11 | Id-pattern counts per golden dir unchanged | P15 script, base vs head: 43 rows / 21,435 occurrences |
| A11b.12 | CDO collision delta reported | the census line at each stage, both universes |
| A11b.13 | Non-speed, no bench (Decision 58) | §8 |
| E14 | CDO ratchets | `scripts/cdo-gate` green on the head |

Invariants touched: I1 (no build counter moves; the `tests/cli` counter test is green), I2 (as
amended by 11B-1), I6 (non-speed), I7 (§4.7), I9 (§5), I10 (C moves findings: `cdo-measure` and
triage, §8). I3, I4, I5, I8 and I11 are untouched.

## 7. False-positive and failure shapes

- **A merge point the trace missed**: A's identity gate cannot see it (A reproduces whatever is
  there), so B would leave it on its old rule. Guards: the round-1 trace closed every path P6
  left open, and `p6b-trace.md` has no unread path left (§3.2). T2 fails for a set-valued
  first-row or last-row choice on the surfaces it compares (§5.2), and the per-operation tests
  of §5.0 pin the rest. No test claims to catch every possible missed point (R1-08).
- **A compat function is not today's rule**: A's identity run fails (F4).
- **A C move outside R-MOVES**: a defect (F2).
- **Node equality accepts a pair the anchor check rejected for a reason other than `#if`
  arms**: P4 says this happens on 0 pairs; T4 pins the check.
- **A row with `node_of == None`** turns every paired site into `caller_mismatch` (the anchor
  check could pass one): 0 such pairs on CDO and every fixture (P4); the loss gates in both
  universes would show it (R1-20).
- **The union merges items that are not equal**: identity is the whole payload (§4.6); T2u.
- **The union widens coverage the wrong way**: the least complete status wins (P8, T1-cov).
- **A single-valued pick hides a row's value**: stated (D6). The pick is per key and the owner
  of a key is always a row that has it (T6). The losing row's value is not kept (§4.8: the
  per-row outputs are consumed); a later per-row reader is step 6's.
- **Pieces of one key from two rows** (an edge from one arm, bindings or frames from the
  other): today's R9/R5 and dependency order index mixes end at B (T6, T1g).
- **Class R detectors keep a first-row finding choice** for a collided id whose arms differ at a
  shared key: stated limit (11B-3), listed in §12.
- **The projection merges findings**: findings are per anchor, and the projection applies to
  per-id tables only.
- **An ix used across models**: the stores are owned by their model's view, so no API takes a
  bare ix from outside.
- **A reader of `typed_edges` through `primary()` left behind**: it no longer compiles.

## 8. Measurement plan (Decision 58: no bench)

There is no `bench_ab.py` run and no wall or peak bound. The CDO evidence is `scripts/cdo-gate`
on the head, plus an identity run at each stage boundary, so that each move is attributed to
one stage.

Setup: `WS` = `cdo_ws` from the main checkout's `.agent/config.json`, read at run time and never
written into a file. `TREE_SITTER_AL_PATH` = the main checkout's grammar. `cdo`-profile binaries
are built from clean detached worktrees (correctness only; no timing is read). The runs go
through `agentflow run --name bench` (the shared machine's slot).

**The scripts are fail-closed (R1-11)**: `/work/bench/160-11b/identity-run.sh LEFT_ALSEM
LEFT_ALDUMP RIGHT_ALSEM RIGHT_ALDUMP OUT EXPECT_JSON` and its checker `idcheck.py`. The
round-1 probe scripts (`run-base.sh`, `run-probe.sh`) are NOT reused for acceptance.
`identity-run.sh` runs under `set -euo pipefail` and stops non-zero on any of these:
- any analyzer exit other than 0;
- an empty output, or one that is not valid SARIF (`runs[].results`) or analyze JSON
  (`payload.findings`), so two failed runs can never compare equal;
- a run count other than the exact expected one (16 CDO + 16 per fixture dir; 3,952 on this
  base);
- a histogram difference;
- any difference that `EXPECT_JSON` does not admit.

It writes one machine-readable, exhaustive diff report per pair (`*.report.json`: both SHAs,
the result counts, whether anything outside the results differs, and every differing result
with its IDENTITY, i.e. rule, fingerprint and location, plus a message-only flag and both
texts), then the `accept` summary. An expectation names, per report, the exact set of
identities that may move, the counter pattern and its delta. Any other moved result fails, of
any rule, including another `d9`. So does a pinned result that did not move, or a changed
identity. The analyze JSON's `generatedAt` is the one field ignored, and only for
`--format json`. Validated on the stage-C prototype (§3.4):
- 0 violations against `expect-C-exact.json`;
- 4 violations, as it must, against `expect-identical.json`;
- the negative substitution fails with the two expected violations.

For each stage boundary (base → C, C → A, A → B), each side:
1. `aldump --program-call-graph-stats $WS` JSON: byte-equal (SHA `c032c8db…a1594ef` at every
   stage).
2. `analyze $WS` at `--scope primary` and `--scope all`, cross-app and `--single-app`, in
   `--format sarif` AND `--format json` (R1-21).
3. Every fixture dir with an `app.json` (246), both universes, both scopes, both formats.
4. `ALSEM_RETAINED_CENSUS=1 analyze`, each universe: the collision line (with
   `groups_mixed_role`) and every bucket, reported.

Why SARIF and JSON cover the other surfaces (R1-21): `--format html`, `pr-summary` and
`terminal` render the same `RunOutput` findings that SARIF and JSON serialize (`gate/run.rs`;
html through `gate/format_html.rs`). `policy check` and `batch` read the same `View` cells,
whose producers are the ones this step changes. The C diff is message-only in two serializations
of the same findings, so a third rendering adds no evidence. `digest` and `fingerprint` are NOT
compared on CDO: their cross-app output may carry the same extra event for routines whose cone
reaches `PageManagement`, and that is C's correction propagating, not a separate move. On the
fixtures their goldens (the `cli` target) must stay identical: Task 1's `scripts/ci-steps task`
checks that (the prototype changed no fixture's analyze output, which is consistent with it but
did not run the goldens). `scripts/cdo-gate` on the head covers the ratchets.

**Reading the result (pre-stated, binding).**
- **C:** `accept` with `/work/bench/160-11b/expect-C-exact.json` (frozen, R-MOVES) must report 0
  violations. That means the 4 cross-app CDO reports (SARIF and JSON, both scopes) differ in
  exactly the 14 pinned identities of §3.4 and in no other result, each one a
  `d9-transaction-span-summary`, message-only, with "publishes N event(s)" +1 and no other text
  changed, and every other report is identical. The census must read
  `j2_alias_nodes=0 j5_compat_rows=0`. The `cdo-measure` skill records the move: findings per
  detector unchanged in count, fingerprint churn 0 (every moved result keeps its fingerprint by
  construction of the identity check).
- **A and B:** `accept` with `/work/bench/160-11b/expect-identical.json` (no admitted difference)
  must report 0 violations.
- Anything else is F2 / F4 / F5.

Numbers go to `docs/measurements/<date>-issue-160-step11b.md`.

## 9. Amendments this spec asks the epic to take

- **11B-1: row 11b's "byte-identical" holds for stages A and B everywhere, and for stage C
  everywhere except the 14 cross-app CDO `d9-transaction-span-summary` results of §3.4 (in every
  serialization of them: SARIF, JSON, and what renders the same findings) and their propagation
  into cross-app `digest` / `fingerprint` of routines that reach `PageManagement`.**
  Reason: the 11b discoveries put the corrections here, and one of them (the caller check)
  restores an event that the not-`CLEAN26` arm of `GetPurchaseHeaderPageID` really publishes.
  The goldens, single-app CDO and the histogram stay identical (P4c).
- **11B-2: the corrections are 11b's stage C**, taken from the ledger discovery "#160 step 11b
  (routine identity corrections)". Its acceptance is A11b.1-A11b.4.
- **11B-3: "string ids a projection" and "per-occurrence in the engine" (epic §5.3) mean, in
  11b: the PRODUCERS emit per row, and one projection makes the per-id inputs BEFORE the
  computations; the cone, the solve, the event graph and every reader stay per string id.** The
  208 `edges_by_from` reader lines are not re-keyed. Per-row cones and solves go to step 6
  (§12). Each reader moves when a later step gives it a reason to.
- **11B-4: the merge inventory is 22 points from P6 plus about 30 from the round-1 trace, in
  eight classes (§3.2), not the epic's three; "one rule" is the full-value union across a
  collided id's rows for set-valued inputs (single-row ids untouched), plus one named pick (the
  LAST OWNING row per key, every piece of a key from that row; keys are call sites, ops, and
  parameter indexes, so a parameter's role or dispatch is never unioned) for single-valued
  lookups.** A row, one call site's edge, or one parameter's role cannot be unioned. Class U's
  collided lists take the union's full-value tie order. Decision 6's "order-independent" holds for
  the set-valued inputs. Guard frames (class G) and the detectors' own finding choice (class R)
  keep today's rule in 11b, each stated with its reason.
- **11B-5: one claim, three stages, C → A → B, each closed by its own fail-closed identity run;
  C is Task 1, mergeable alone.**
- **11B-6: the Primary SCC is unchanged.** Tarjan already skips repeated ids (P7). The k-fold
  copies P6 found outside the SCC (reverse graph, r3a1) end at B.
- **11B-7: the event graph's `None` arm stays as a fail-closed guard (D7), and A8 pins it by a
  hand-stated `None`**, because no natural unmapped row remains (P13).
- **11B-8 (R1-13): the epic's interim string keys end per reader, not at step 11.**
  - Decision 25 ("interim key = the model's string id until step 11") becomes: "until the step
    that re-keys that reader". Those steps are: step 6 for the `pd` arena, the call-site ledger
    and per-row cones and solves; step 8 for `dep_routine_ids` (`Role`); later steps for any
    detector (class R) or `View` cell they re-key.
  - Epic §5.4 "rows gain `ix`" is met by position (11A-2): a row stores no `ix`; `CallEdge`
    stores `from_ix` and the per-row producer outputs are indexed by `RoutineIx` (§4.3).
  - Epic §5.3 "per-occurrence in the engine with one deterministic projection merge" is read as
    11B-3 and 11B-4 say.
  - Epic §5.9 ("before step 11b every cell keys routines by the model's string id") is amended
    the same way: after 11b the cells still key by string id, until the reader's step.

## 10. Fallbacks (every "if the measurement shows X")

Every fallback that changes code re-runs every identity gate it could affect before the claim
continues. "Stop at X" means nothing after X is committed; a stage that has passed its own gate
stays, and the ledger records the reason.

| # | If | Then (applied without a new panel round; recorded in the ledger) |
|---|---|---|
| F1 | The panel refuses 11B-1 (it wants byte identity at C too) | C leaves this claim and is filed as its own issue (the discovery, unchanged); 11b = A + B, both predicted byte-identical (§3.5) |
| F2 | C's identity run: `accept` with `expect-C-exact.json` reports any violation, or a census value ≠ 0, or the loss gate ≠ the residual minus 5 | A defect: the implementation is not the prototype's rules (`probe-final.patch`). Diff it against those rules, fix, re-run. Not closed in one attempt → stop at the base (nothing merges) |
| F3 | The XmlPort fix (or any other change to the base) merges before C | Before C starts, the prototype patch is re-applied to the new base and the fail-closed identity run is repeated. **Admissible** (source-based, binding): only results that are `d9-transaction-span-summary`, message-only, whose only change is "publishes N event(s)" +1, and whose span root's cone reaches `PageManagement.GetPurchaseHeaderPageID` (the one call the correction restores is its first arm's `OnAfterGetPurchaseHeaderPageID`, source line 351). `expect-C-exact.json` is then re-pinned (`idcheck.py pin`) from that run only if every moved identity passes the admissibility test above, the root list in §3.4 is updated, and the new R-RESIDUAL (the residual minus the 5) is the loss gate. Any other difference → the base change interacts with C: stop, re-panel C |
| F4 | A's identity run fails (`accept` with `expect-identical.json` reports a violation, or a golden, or a `--lib` outcome outside §5.9) | A compat function is not today's rule. Bisect A's commits to the point, fix that function, re-run. Not closed in one attempt → stop at C's head (C alone may merge: its gate passed) |
| F5 | B's identity run reports a violation (CDO or a fixture) | §3.5's prediction is wrong for some point. Each moved result is triaged to a §3.2 point and a collided id. **Admissible** (source-based, binding): a move that is exactly the union (S/K) or the LAST owning pick (P) applied to that collided id's rows as read from the AL source, or the propagation of such a move into a caller's result (a result whose subject's cone, witness or callee chain contains the collided id). The triage uses the `golden-diff-triager` agent for a golden, and `cdo-measure` + `triage-findings` for CDO. Every admitted move is listed in the numbers file with its source reading, and `expect-B.json` is written from that list. Anything else is a defect. ONE fix attempt; not closed → stop at A's head (C and A may merge) |
| F6 | A merge point is found during implementation that §3.2 does not list | It joins the class whose rule fits (union for set-valued, LAST owning for single-valued, or "kept" with its reason), recorded in §3.2 and the ledger without a new round. If it needs a third rule beyond union and LAST owning (or a new "kept" exception), the claim stops and the spec goes back to the panel |
| F7 | A census bucket grows more than 20 % at any stage | Name it; if a per-row output outlives its projection, make the projection consume it (§4.8); re-run the census once. Still > 20 % → reported in the numbers file and raised to the final panel (not a block: 11b has no peak bound) |
| F8 | Making a point's producer per-row forces edits in more than 40 reader files | Make the producer per-row only; the readers keep the projected per-id value (11B-3); state it in §3.2 |
| F9 | (decided, Q1) Twin-node fan-out | Not 11b: 10 routes on CDO (P10), handed to step 6 (§12); D2 stays |
| F10 | (measured: true) No natural collided row differs in facts or coverage, so B has no CDO witness | Recorded as a stated limit: §5's fixtures (every operation of §5.0's table with its mutation, and T2's set-valued swap) are the evidence of the rule |
| F11 | T6b's single-row case or B's identity run shows that routing `d37` / `compose_roles_only` through `site_pick` changes an answer for a SINGLE-row id (§4.6's equality of "first targeted `CallEdge`" and "first `CombinedEdge`" fails somewhere) | The accessor keeps today's edge choice (the first `CombinedEdge` with that call-site id) for single-row ids and uses the consistent pick only for collided ids. The exception is stated in §4.6 with the differing shape, and a test pins it (that single-row shape through d37 and `compose_roles_only`, today's answer). B's run is repeated once; still not identical → F5 |

## 11. Decisions taken in round 1, risks, and what is left open

Decided (conductor, round 1; recorded so they are not reopened without new evidence):
- **Q1**: one row (D2, today's anchor row). Twin-node fan-out goes to step 6 (§12).
- **Q2**: least complete wins (complete < partial < unknown); reasons are a sorted union; tested
  through the production consumers (T1-cov).
- **Q3**: no model tag. `rows_of_id` is transient, and every per-row output lives inside one
  model's projection (§4.3).
- **Q4**: amendment 11B-8.
- **Q5**: `Role` stays in step 8; role homogeneity is asserted and counted (§4.3, T9).
- **Q6**: the LAST OWNING row per key, every piece of a key from that row, with the within-row
  candidate policy kept (§4.6). No id format change.
- **Q7**: one claim; C is Task 1, committed and identity-checked on its own and mergeable alone.

Risks:
1. **A node-equality caller check is only as right as `node_of`.** `node_of` comes from the
   physical row (workspace) and from `source_routine_node_id` (dependency source). A wrong value
   would now pair silently where the anchor check used to fail. T4 pins the check; the loss gates
   in both universes catch an error across the population.
2. **A merge point the trace missed.** T2 catches a set-valued one on the surfaces it compares; a
   missed single-valued one keeps its old rule (F6 decides).
3. **Class R** detectors (d1, d2, d3, d13, d18, d47, d49, d51, the gate's `kept_ids`) keep their
   first-row choice when a collided id's arms differ at a shared key (§12).
4. **B has no natural CDO witness** (F10).

Left open for the panel: none of Q1-Q7. One question remains:
- **Q8.** Should the class R detectors' first-row choices be listed only (§12), or should 11b move
  each one onto the LAST owning row now, as a reader change? That would widen 11B-3, and it
  would touch d1's dataflow (`d1_dataflow.rs:3164-3322`) and cohort code.

## 12. Discoveries filed with this claim (step 6 and later)

Recorded in the ledger's `## Discoveries`:
1. **Per-row cones and per-row solves** (R1-05): the cone and the L4 solve stay per string id in
   11b. A per-row cone must define the in-SCC case (an in-cycle row's cone includes its siblings'
   facts) and the frame rule; it belongs with step 6's `pd` arena (G-4).
2. **Guard frames of alternative arms are conjunctive requirements** (R1-02;
   `engine/l4/param_guard.rs:37-73`). Today a collided id takes the LAST arm's frame (R2d) and
   the span frames are first-wins (#15). Unioning opposite requirements describes an impossible
   path, and LAST can prune another arm's commit. The fix keeps requirements per row through the
   per-row cone (with discovery 1).
3. **Twin-node fan-out** (Q1): an edge into a node whose rows have several string ids
   (`GetAccessToken` / `GetFreshAccessToken`, 10 routes on CDO, P10) goes to one row. Per-row
   fan-out is step 6's call-site ledger.
4. **Class R reader choices** (the trace, §3.2): detector-level first-row picks over a collided
   id's rows (d1 groups / direct ops / terminal sink, d2, d3, d13, d18, d47, d49, d51,
   `gate/run.rs` `kept_ids`). They are harmless when the arms agree on shared keys, and they pick
   the first arm when the arms differ. They belong to the step that re-keys those readers.
