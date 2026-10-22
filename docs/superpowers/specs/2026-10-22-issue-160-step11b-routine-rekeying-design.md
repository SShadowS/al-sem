# #160 child spec: step 11b — routine identity corrections, `RoutineIx` re-keying, the one collided-id rule

Status: draft for spec panel round 1 (2026-10-22). Made from the pre-draft
`/work/bench/160-11b/draft-11b-design.md` once the probes had run. The probes changed four
things in it; §0 lists them.
Child of `2026-10-18-issue-160-analysis-session-design.md` (the epic): §1 goal 7, §5.3, §5.4,
§5.5 (M1, M2, M7), §5.11, §9 row **11b**, E2, E11, E14, I2, I6, I7, I9, I10, Decisions 6, 25,
39, 43, 57, 58. It builds on `2026-10-22-issue-160-steps10-11a-siteix-routine-identity-design.md`
("10+11a" below): its §4.6-§4.10, its amendments 11A-2 to 11A-4, its §5.10, and the two ledger
discoveries that name 11b (the identity corrections; A8's precondition). The shape follows
10+11a and `2026-10-21-issue-160-step7b-closed-sets-fold-memo-design.md`. Where this document
and the epic disagree, this document is wrong, EXCEPT for the numbered amendments of §9
(11B-1 to 11B-7). Each amendment gives its reason; until the epic takes it, it is an open
question for the panel.

Base: master `5c42b8f3`, worktree `/work/al-sem-issue-160-a5`. Compared with the 10+11a head
`3b199117`, it adds only the grammar bump #207 (`tree-sitter-al` `960ed312`; the lowerer, the
vocabulary, the cache stamp) and an agentflow change. No file this design edits has moved, and
the CDO histogram SHA and collision line did not change (P1). Every `file:line` below is in that
tree. Every fact the design rests on is a row of the ledger's `## Assumption probes` table
(`.agent/issue-160/ledger.md`, P1-P16). The probe patch, the scripts and the raw output are in
`/work/bench/160-11b/` (`probe-final.patch`, `run-base.sh`, `run-probe.sh`, `p6-inventory.md`).
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
  (§3.2; there are 22, P6).
- **Projection** = the string-keyed view built once from a per-`RoutineIx` table by the one
  rule (§4.6).
- **Stage C / A / B** = the three parts of this claim, in that order (§4.1).

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

## Feasibility first

| Item (epic row 11b + the 11b discoveries) | Verdict | Evidence (ledger row) |
|---|---|---|
| Discovery: `j2_alias_nodes` = 0 and `j5_compat_rows` = 0 on CDO | **feasible**, measured on a prototype | under the stage-C prototype the CDO line reads `j2_alias_nodes=0 j5_compat_rows=0` (P4c) |
| Discovery: the 5 `caller_mismatch` sites leave R-RESIDUAL | **feasible**, measured | anchor and node checks disagree on exactly those 5 and nowhere else (P4); under C the losses are `shape_mismatch 18, adapter_l3_fallback_sites 18` (P4c, P14) |
| Discovery: A8 restated, break 8 still fails | **feasible** with a hand-stated `None` (§5.5) | P13: no natural unmapped row exists after C |
| String ids a projection | **feasible, scoped** (11B-3): producers and merge points are keyed by `RoutineIx`, readers keep a string projection built once | 208 `edges_by_from` lines in 40 files (P9) stay on the projection |
| `ExecRelation` carrier split (M1, M2) | **feasible** | `exec_relation.rs:17-29` reserves it; `.typed_edges` field reads outside the assembly: 56 lines, 10 files (P9) |
| Per-`RoutineIx` stores at every merge point | **feasible, large**: 22 points in 4 classes (§3.2) | P6 |
| The collided-id rule, fixture flipped | **feasible** | `collision_rules_hand_stated` (`detector_context.rs:2393`) pins today's rules |
| Byte-identical (epic row) | **feasible for stages A and B everywhere; stage C everywhere except 14 cross-app CDO SARIF results** (11B-1) | P4c: 8 golden-bearing corpora unaffected (0 of 984 fixture SARIF comparisons differ), single-app CDO identical; P12 for B |
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
2. **Stage A, the re-keying.** `RoutineIx` becomes a stored type. Every merge point keys by it.
   `ExecRelation` carries the Primary and Cone payloads as separate fields, so `primary()` can
   no longer reach the typed edges. String-keyed views are built once by a projection that, in
   this stage, reproduces each store's current rule exactly. The stage is byte-neutral.
3. **Stage B, the one rule.** The projection becomes the epic's one deterministic merge
   (Decision 6). Set-valued tables take the union, independent of order. Single-valued lookups
   take one named pick. The duplications listed in §0.2 end.

Why: epic §5.3. Today two rows of one id are merged by whatever rule the store they meet in
happens to use. That gives a collided routine's summary one arm's direct facts and both arms'
callees, a combination no build of the code has (P6, R2a + R1). And three of the engine's
answers are known to be wrong (10+11a §4.7-§4.9).

## 2. Non-goals

- **No id format change.** `compute_routine_id`, `stable_routine_id`, `{id}/cs{n}` and
  `{id}/op{n}` keep their bytes. A collided id stays collided as a string.
- **No change to resolution.** The program graph, `RoutineNodeId` and the histogram stay as
  they are (SHA equal at every stage, both universes).
- **The XmlPort table-element discovery is not fixed here.** It covers the 18 `shape_mismatch`
  sites and has its own issue (F3).
- **`Role` stays in step 8.** `dep_routine_ids` (69 sites) keeps its string key (Q5).
- **Detector readers are not re-keyed** (11B-3). Only producers and merge points are.
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

### 3.2 The merge points (P6, `/work/bench/160-11b/p6-inventory.md`)

The 22 points fall into five classes. The class decides what stage A keys and what stage B's
projection does.

| Class | Points | Today | After A (engine key) | B's projection |
|---|---|---|---|---|
| **U** already a union, or the result does not depend on row order | R1 `edges_by_from` / `edges_from_order` (`combined_graph.rs:418-428`); #10 `uncertainty_edges_by_from` (`detector_context.rs:1597-1605`, `summary_runner.rs:1427-1431`); #12 entry points (`entry_points.rs:63-115`); R3b `uncertainties_by_node`, `parameter_roles_by_routine` (`detector_context.rs:1669-1696`) | union / id-pure | per-row lists | union (no change) |
| **S** set-valued but merged by a non-union rule | R2a `det_cone_inputs` (`detector_context.rs:1252-1261`); R2b cross-app base (`capability_cone.rs:3936-3961`); R2c r3a3 builders (`:4368-4397`, `:4565-4577`, `:4726-4735`); R2d `guard_frames` (`:2383-2394`); R3a summaries drain (`detector_context.rs:1385-1496`); #13 v2 solve inputs `base_summaries`, `dispatch_by_id` (`summary_runner.rs:1362-1393`); R4 event `processed` (`events.rs:366-376`); R4b one `EventSymbol` per publisher row (`:313-320`); R1c interface candidates (`combined_graph.rs:573-672`); #14 `commits_by_routine` (`transaction_spans.rs:487-507`); #16 checked-run seeds (`:598-625`); #17 `event_param_temp` index (`event_param_temp.rs:59-69`) | last / first / per-row duplicate | per `RoutineIx` | union (§4.6) |
| **P** single-valued: a row, a call site's one edge or binding list, a modifier | R1b (`combined_graph.rs:283-331`), R1d (`:541-550`), R5 `upgraded_bindings` (`program_calls.rs:1352-1381`), R6 `routine_by_id` (`:1336`), R9 `first_resolved_edge_per_callsite` (`detector_context.rs:2028-2037`), #11 `access_modifiers` (`:1527-1535`), #13's `routines_by_id`, `body_avail_by_id`, `stable_map` (`summary_runner.rs:1349-1402`), #15 span `frames` (`transaction_spans.rs:148, :380`), #19 d46/d48/d61 local maps, #20 snapshot anchors (`snapshot.rs:1405-1422`, `:1777-1986`, `:2349`, `:2437`), #21 lookup indexes (the 14 `collect`s P6 lists), #22 `OperationJoin` (`effect_query_cli.rs:462`) | mostly last, some first | per `(RoutineIx, call-site index)` where the key is a call site; otherwise unchanged | one pick: the LAST row in `RoutineIx` order (§4.6) |
| **F** fail-closed | #18 `closed_world_temp` `dup_ids` (`closed_world_temp.rs:105-110, :198, :254`) | a duplicated id is never proven | unchanged | unchanged (still fail-closed; the projection gives it the id's rows) |
| **K** k-fold copies from the duplicate ids in `CombinedGraph.nodes` (`combined_graph.rs:415-416`) | `reverse_call_graph.rs:37-41` (+ `pending_writes.rs:43-50`, spans, closed-world temp); `project_r3a1` `combined_edges` (`combined_graph.rs:1111-1115`); snapshot per-row reads (`snapshot.rs:1353-1385`, `:1862-1921`, `:2311-2338`) | an edge or fact repeated k times | `nodes` per `RoutineIx` (k distinct entries) | the projection lists each id once |
| kept | R7 the solver interner (`routine_interner.rs:96-108`); R8 `ConeDerivedStore` (one per SCC member: Tarjan skips repeated ids, `scc.rs:69-70`) | — | renamed `SolveIx` / unchanged | fed by the projection |

Not traced by P6, so the plan reads them before Task A: `d1_graph.rs:134`, `d1_dataflow.rs:591`,
`detector_context.rs:1744-1751`, `gate/*` beyond the three maps, `ordering_inter.rs`,
`unresolved_cone.rs`, `dep_artifact_l4.rs:956-990` (§7).

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

### 3.5 What stages A and B are predicted to move

Nothing on CDO, in either universe, and nothing in any golden:
- A reproduces every store's current rule (§4.5).
- B changes rules only for collided ids. CDO's groups have equal direct facts and coverage
  (P12), their edges are already unioned (class U), and they are dependency rows, so they are
  in no Primary payload's `from`. That leaves classes K and P with nothing that differs on CDO
  (single-valued items of equal rows). The corpus group (two bodiless rows) is the same.
B's real move is on hand-stated fixtures only (§5). If the B identity run moves CDO output,
that disproves the prediction (F5).

## 4. Design

Decisions: **D1** three stages C → A → B (§4.1); **D2** the corrections read `node_of` (§4.2);
**D3** `RoutineIx` stored, `SolveIx` (§4.3); **D4** carrier split (§4.4); **D5** per-row
engine keys, string projection for readers, compat projection in A (§4.5); **D6** the one rule:
union for set-valued, LAST row for single-valued (§4.6); **D7** keep the event graph's
`unmapped` branch as a fail-closed guard (§4.2, §5.5).

### 4.1 Three stages, each with its own output (D1)

Order inside the claim: **C, then A, then B.** Each stage is one or more commits, green on
`scripts/ci-steps task`, and each is closed by its CDO identity run (§8) before the next one
starts.
- **C first**: it is small (three reads), it runs on today's string-keyed engine, and it is the
  only stage whose output moves. That move (§3.4) is measured on its own, against the base.
- **A next**: the large mechanical diff, held to byte identity against C's head.
- **B last**: the rule change, measured against A's head. It is predicted byte-identical on CDO
  (§3.5).

### 4.2 Stage C: the corrections read `node_of` (D2)

- **J2 → node-filtered J2.** `j2_row[n]` = the first non-symbol-only row at `n`'s surface
  anchor **whose `node_of` is `n`**; absent if there is none. The `first_at` map
  (`workspace.rs:2168-2190`) keeps all the rows at each anchor in row order, and the J2 loop
  (`:2212-2225`) picks the first with the node. This equals today's J2 on every node that is not
  an alias, so the change is exactly the alias set (P3: 2 routes). It is not "the first / last
  row of the node": on the `GetAccessToken` shape (one node, two ids at one anchor) "last" would
  move `CallEdge.to` on all 10 routes P10 lists.
- **The caller check → node equality.** `row_nodes.node_of[ri].as_ref() != Some(&ce.edge.from)`
  means `caller_mismatch`. The key join has already paired one expression on both sides, and an
  expression belongs to exactly one declaration, whose node is `node_of`. P4: this disagrees
  with the anchor check on exactly the 5 R-RESIDUAL sites. `decl_anchor`, its build loop
  (`workspace.rs:2193-2210`), and `join_reads`' call from `RowNodes::build` are deleted.
- **J5 → `node_of`.** `j5_node` is deleted. `program_event_graph` reads `node_of[ri]` for
  subscriber rows. The `processed` rule (R4) is unchanged in C. **D7:** the `None` arm
  (`events.rs:370-373`) stays as a fail-closed guard: no edges, counted as `unmapped`. P13 found
  no natural `None` after C, but a dependency row of an app the graph does not know has no node
  (`append_dependency_rows`; 10+11a FP-03), so the guard is not dead by construction.
- **The census.** In C, `j2_alias_nodes` and `j5_compat_rows` are computed against the
  corrected answers and must read 0 on CDO (§8). They are deleted in A, together with the
  fields they compare.
- **The CDO loss gate.** `adapter_cross_app_losses_on_cdo` (`tests/program_resolve_harness.rs:10707`)
  asserts R-RESIDUAL minus the 5 `caller_mismatch` sites: 18 `shape_mismatch` and 18
  `adapter_l3_fallback_sites` (P4c), or whatever the XmlPort fix leaves (F3).

### 4.3 Stage A: `RoutineIx`, stored (D3)

- `pub struct RoutineIx(pub u32)` goes in `src/program/model/` and derives `Copy`, `Ord` and
  `Hash`. Its value is the row's position (the same meaning as 11A-2). A row does not store it;
  the re-keyed tables do. `CallEdge`, `CombinedEdge` and `TypedEdge` gain `from_ix: RoutineIx`.
  It is not serialized: `CallEdge` is not `Serialize` (`calls.rs:37`), and the other two are
  checked by the plan. It is set where `from` is set.
- `ModelEntities` gains `rows_of_id(&str) -> &[RoutineIx]`, built once at the end of assembly.
  It is the one place that knows which rows share an id. Every projection reads it.
- The L4 interner's type is renamed `SolveIx` (11A-4). This is mechanical, in 12 files (P9).
  `alsem query` output does not change (`cli-query-goldens`).
- An ix means nothing in another model (epic §5.4: one model per universe). No tag is added
  (Q3).

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

### 4.5 Stage A: per-row engine keys, compat projection (D5)

For every point of classes U, S, P and K in §3.2, the producer keys by `RoutineIx` (or by
`(RoutineIx, call-site index)` for a call-site key). The string-keyed table its readers use is
then built ONCE by a projection over `rows_of_id`:
- **In stage A, each projection reproduces that point's current rule**: union for U; last,
  first, drain-by-first or per-row duplicate for S; last or first for P; k copies for K. These
  are the compat projections. There is one small function per rule, and each has a doc line
  naming the rule it reproduces and the point it serves. That is what makes A byte-neutral: it
  changes where rows are kept apart, not what is emitted. Stage A's identity run (§8) is the
  proof.
- **The cone composes per occurrence.** `det_cone_inputs` and the cross-app and r3a3 base
  builders produce per-`RoutineIx` direct facts and coverage. The cone walk takes per-row
  inputs and per-row adjacency (`from_ix`). Row A's inherited facts are then A's callees only.
  An edge's `to` is a string id, and an edge into a collided id is an edge into each of its
  rows (today's meaning; Q1).
- **Readers are not re-keyed** (11B-3). A detector that reads `edges_by_from[id]` or
  `ctx.routine_by_id[id]` reads the projection.

### 4.6 Stage B: the one rule (Decision 6) (D6)

Stage B replaces the compat projections with two rules:
- **Set-valued (classes U, S, K): union, independent of row order.** Edge and uncertainty lists
  are deduped by today's sort keys (`edge_sort_key`, `uncertainty_sort_key`) and sorted. Fact
  lists are deduped by `inherited_fact_key` and sorted. Flags are OR-ed. Reasons are a sorted
  union. Coverage status takes the least complete value: direct `complete` < `partial` <
  `unknown`, inherited `complete` < `partial` (P8). Events: one `EventSymbol` per event id, and
  each subscriber id emits the union of the subscriptions of its rows' distinct nodes, deduped,
  in first-row order (equal to today whenever an id's rows share a node: every CDO and corpus
  group). Interface candidates: one candidate per distinct implementer. Transaction spans: one
  span per call-site id, and the commit seeds are the union of the rows' commits. The
  `event_param_temp` rewrite applies to every row of the id. The reverse graph and r3a1 list
  each id's edges once.
- **Single-valued (class P): the LAST row of the id in `RoutineIx` order.** `rows_of_id(id).last()`
  serves every lookup index, the per-call-site bindings and edge pick (R5, R9), the access
  modifier, span frames, the snapshot anchors and `OperationJoin`. LAST is today's rule at most
  of these points (P6: 14 of the `collect` indexes, R5, R6, R1d, #11, #13, #19, #20). The points
  that are first-wins today (R1b, R9, #15, #22) change to it. A single row or a single call
  site's edge cannot be unioned, so this is a deterministic pick, not an order-independent
  merge (11B-4, Q6).
- **R7** (the solve's interner) keeps one node per id and is fed the set-valued projection.
- **Class F** is unchanged.

### 4.7 Determinism (I7)

`RoutineIx` order is today's row order. Projections walk rows in `RoutineIx` order and sort
their output. Union is commutative and idempotent, so set-valued output does not depend on which
row comes first (T2). Single-valued picks depend on row order by design, and row order is fixed
by assembly (workspace files, then ABI rows, then dependency source files, in path order).
`rel.typed` keeps today's concatenation (Decision 43). The Primary SCC never sees duplicates:
Tarjan skips repeated ids (P7), so `rel.scc` and every serialized SCC count are unchanged.

### 4.8 Memory

| Item | Change | CDO cross-app |
|---|---|---|
| `RowNodes.decl_anchor`, `j5_node` (C) | deleted | the `program.row_nodes` bucket is 8,809,976 B today (P11); expect roughly 4-5 MB less |
| `from_ix` on three edge types (A) | +4 B per edge (with padding, up to 8 B) | CallEdge ≈ 84,717 (10+11a P15); combined and typed edges counted by the census |
| per-row stores beside their projections (A) | for an id with one row the projection MOVES the value, it does not clone it; only the 4 collided rows are held twice | ≈ 0 |
| carrier split | `typed` moved, not copied | 0 |

11b has no peak bound (§5.12 does not list it). The census is read per stage and reported (§8,
F7).

### 4.9 What is deleted

`RowNodes.decl_anchor` and `j5_node`, and their build code (C); `CollisionCensus.j2_alias_nodes`
and `j5_compat_rows` (A); `Converter::routine_by_id` (A: `AbiRowIds` maps node → `RoutineIx`);
the R3a skip block and its comment (`detector_context.rs:1412-1468`) and the `debug_assert` on
how cones and `direct_full` collide (B; I2's exception, §5.9); the name `RoutineIx` in `l4`
(renamed).

## 5. Test plan

Every new or restated test lists its break (made with Edit, reverted with Edit), the failing
assertion and the revert, and both outcomes are recorded (I9). A scripted break asserts that it
applied.

### 5.1 T1: the hand-stated collision fixture flips (the epic's proof row)

`collision_rules_hand_stated` (`detector_context.rs:2393`). It passes UNCHANGED through C and A
(A's gate: the compat projection is today's rule). At B it is restated:
- the projected direct facts are A ∪ B;
- the coverage is the least complete of the two;
- the summary's inherited half has both callees;
- `edges_by_from` has both rows' edges;
- per row: row A's derived row holds A's callee and not B's.

Breaks: projection first-wins → "B's facts in the union" fails; last-wins → "A's facts" fails.
Revert.

### 5.2 T2: order independence

The T1 fixture with its two rows swapped in `ws.routines`: every set-valued projected artifact
is serde-JSON-equal to T1's. Break: make the set-valued projection "first row" → the two orders
differ → fails. Revert.

### 5.3 T1b-T1f: one hand-stated fixture per class S/K duplication (P6's new findings)

Each forces two rows onto one id by assignment (`force_shared_id`, `detector_context.rs:2051`)
and asserts B's rule. Each has its break.

| Test | Shape | B's assertion | Break → fails |
|---|---|---|---|
| T1b `collided_publisher_is_one_event_symbol` | two publisher rows of one id | one `EventSymbol`, one dispatch edge per subscriber | push a symbol per row |
| T1c `collided_interface_call_counts_each_implementer_once` | both rows make one interface call | `candidate_count` = the number of implementers | union without dedup |
| T1d `collided_id_is_one_reverse_entry` | a collided caller | `reverse_call_graph` lists each edge once; r3a1 `combined_edges` once | walk duplicate `nodes` |
| T1e `collided_commit_seeds_its_own_rows` | one row commits, the other does not | the span seeds from the union of commits, one span per call-site id | seed `routine_by_id[id]` (last row) |
| T1f `event_param_temp_rewrites_every_row` | a subscriber group | both rows get the temp rewrite | index the last row only |

At C and A each of these tests asserts TODAY's behaviour (that is A's identity on a hand-stated
input). At B it is flipped to the assertion above, in the same commit as the rule.

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
  `event_graph_reads_node_of`: in the T8a shapes, each subscriber row emits its own node's
  subscriptions, and the arm that was unmapped now maps. Break: map a row to the first node at
  its span (the old rule) → the two-node shape gives node A's subscriptions to row B → fails.
- **A8** (`tests/r4/r4_cross_app_program.rs:1743`, the ledger discovery): the precondition is
  restated on the corrected mapping (both rows of `H` map to ONE node, both `Some`), and the
  each-edge-once assertion is unchanged. Break 8 ("processed marked before a row maps") is
  vacuous once both rows map (P13), so A8 adds a second case. Through the `#[doc(hidden)]` entry
  (`cross_app_rows_before_attach`, `program_calls.rs:878`) it takes the rows, sets
  `node_of[first] = None` by assignment, builds the event graph, and asserts that `H` still
  emits each edge once. Break 8 fails that case. Revert. That is D7's guard, pinned.
- `j5_compat_rows_counts_the_subscriber_gaps_only` is deleted with the counter (A).

### 5.6 T6: call-site id collisions (R5, R9)

The T1 fixture with each row's call site 0 resolving to a different target. Engine: `(A, 0)` and
`(B, 0)` keep their own resolved edge and bindings. Projection for `{id}/cs0`: row B's (the
last). Break: make the projection first → fails. Revert.

### 5.7 T7: `from_order` with a collision group

d17's two-caller fixture (step 4) plus a collided caller: the projected sequence equals today's
(first appearance per id). Break: no dedup → the id appears twice → fails. Revert.

### 5.8 T8: the census after C, and the CDO loss gate

`collision_census_counts_groups` (`program_calls.rs:3197`) is unchanged. C's CDO run prints
`j2_alias_nodes=0 j5_compat_rows=0` (§8), and then the fields go in A.
`adapter_cross_app_losses_on_cdo` asserts the residual minus the 5; its discrimination is
re-recorded (restore the anchor check → the 5 reappear → fails).

### 5.9 Tests deleted or rewritten with the code they pin (I2's exception)

- Rewritten: T3's J2 test; T4's two caller tests; T5's J5 test and A8; T1 and T1b-T1f (at B);
  `hand_stated_id_collision_keeps_a_real_summary_and_derived_row` (`detector_context.rs:2302`)
  and `colliding_ids_keep_the_full_summary_union_not_just_the_edges` (`:2811`), which encode R3a
  and are restated to the union at B on the same fixtures.
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
| A11b.4 | C moves exactly R-MOVES | C's identity run: cross-app SARIF differs at each scope in exactly the 14 `d9-transaction-span-summary` results of §3.4, each only "publishes N event(s)" +1; single-app, every golden target, histogram SHA identical |
| A11b.5 | `RoutineIx` stored; `SolveIx` | §4.3; `rg -w RoutineIx src/engine/l4` hits only the model's type |
| A11b.6 | Carrier split (M1, M2) | §4.4; `PrimaryGraph` has no `typed_edges` field |
| A11b.7 | Every class U/S/P/K point keyed per `RoutineIx`, projected once | §4.5; the plan's per-point checklist from §3.2 |
| A11b.8 | Stage A byte-neutral | the 8 golden targets and the `--lib` outcomes (apart from §5.9), CDO SARIF at both scopes in both universes, histogram SHA: all identical to C's head |
| A11b.9 | The one rule | T1 (both breaks), T2, T1b-T1f, T6, T7 |
| A11b.10 | Stage B byte-identical outside the hand-stated tests | B's identity run against A's head: everything identical (§3.5) |
| A11b.11 | Id-pattern counts per golden dir unchanged | P15 script, base vs head: 43 rows / 21,435 occurrences |
| A11b.12 | CDO collision delta reported | the census line at each stage, both universes |
| A11b.13 | Non-speed, no bench (Decision 58) | §8 |
| E14 | CDO ratchets | `scripts/cdo-gate` green on the head |

Invariants touched: I1 (no build counter moves; the `tests/cli` counter test is green), I2 (as
amended by 11B-1), I6 (non-speed), I7 (§4.7), I9 (§5), I10 (C moves findings: `cdo-measure` and
triage, §8). I3, I4, I5, I8 and I11 are untouched.

## 7. False-positive and failure shapes

- **A merge point P6 did not trace** (§3.2's list of untraced points): A's identity gate cannot
  see it, because A reproduces whatever is there, so B would leave it on its old rule. Guard:
  the plan reads those paths before Task A and adds every one it finds to the §3.2 table; T1-T2
  run through `build_detector_context`, the path most points sit on.
- **A compat projection is not today's rule**: A's identity gate fails (F4).
- **A C move outside R-MOVES**: a defect (F2).
- **Node equality accepts a pair the anchor check rejected for a reason other than `#if`
  arms**: P4 says this happens on 0 pairs; T4 pins the check.
- **The union widens coverage the wrong way**: the least complete status wins (P8).
- **A single-valued pick hides a row**: stated (D6); the per-row engine keeps both, so a later
  per-row reader can see both.
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
through `agentflow run --name bench` (the shared machine's slot). The scripts are
`/work/bench/160-11b/run-base.sh` and `run-probe.sh`, adapted.

For each stage boundary (base → C, C → A, A → B), each side:
1. `aldump --program-call-graph-stats $WS` JSON SHA: equal to `c032c8db…a1594ef` at every stage.
2. SARIF, `analyze $WS --format sarif` at `--scope primary` and `--scope all`, cross-app and
   `--single-app`: `cmp`.
3. `ALSEM_RETAINED_CENSUS=1 analyze`, each universe: the collision line and every bucket,
   reported.
4. Every fixture dir with an `app.json` (246), both universes and both scopes: SARIF `cmp`, the
   same loop P4c ran.

**Reading the result (pre-stated, binding).**
- **C:** cross-app CDO SARIF must differ in exactly the 14 results of §3.4 (the same rule id,
  the same span roots, each only the event count +1). Every other byte at both scopes must be
  identical. Single-app, every fixture comparison and the histogram must be identical. The
  census must read `j2_alias_nodes=0 j5_compat_rows=0`. The `cdo-measure` skill records the
  move (findings per detector: unchanged in count; fingerprint churn: 0, as on the prototype, where
  every result's `ruleId` and `partialFingerprints` were identical and only the message text
  moved).
- **A and B:** everything identical.
- Anything else is F2 / F4 / F5.

Numbers go to `docs/measurements/<date>-issue-160-step11b.md`.

## 9. Amendments this spec asks the epic to take

- **11B-1: row 11b's "byte-identical" holds for stages A and B everywhere, and for stage C
  everywhere except the 14 cross-app CDO `d9-transaction-span-summary` results of §3.4.**
  Reason: the 11b discoveries put the corrections here, and one of them (the caller check)
  restores an event that the not-`CLEAN26` arm of `GetPurchaseHeaderPageID` really publishes.
  The goldens, single-app CDO and the histogram stay identical (P4c).
- **11B-2: the corrections are 11b's stage C**, taken from the ledger discovery "#160 step 11b
  (routine identity corrections)". Its acceptance is A11b.1-A11b.4.
- **11B-3: "string ids a projection" means producers and merge points key by `RoutineIx`, and
  readers keep a string projection built once.** The 208 `edges_by_from` reader lines are not
  re-keyed. Each moves when a later step gives it a reason to.
- **11B-4: the merge inventory is 22 points in five classes (§3.2), not the epic's three; "one
  rule" is union for set-valued tables plus one named pick (the LAST row) for single-valued
  lookups.** A row, or one call site's edge, cannot be unioned. Decision 6's "order-independent"
  holds for the set-valued tables, which are all the tables the rule merges.
- **11B-5: three stages, C → A → B, each closed by its own CDO identity run.**
- **11B-6: the Primary SCC is unchanged.** Tarjan already skips repeated ids (P7). The k-fold
  copies P6 found outside the SCC (reverse graph, r3a1) end at B.
- **11B-7: the event graph's `None` arm stays as a fail-closed guard (D7), and A8 pins it by a
  hand-stated `None`**, because no natural unmapped row remains (P13).

## 10. Fallbacks (every "if the measurement shows X")

Every fallback that changes code re-runs every identity gate it could affect before the claim
continues. "Stop at X" means nothing after X is committed; a stage that has passed its own gate
stays, and the ledger records the reason.

| # | If | Then (applied without a new panel round; recorded in the ledger) |
|---|---|---|
| F1 | The panel refuses 11B-1 (it wants byte identity at C too) | C leaves this claim and is filed as its own issue (the discovery, unchanged); 11b = A + B, both predicted byte-identical (§3.5) |
| F2 | C's identity run: any difference outside §3.4's 14 results, or a census value ≠ 0, or the loss gate ≠ the residual minus 5 | A defect: the implementation is not the prototype's rules (`probe-final.patch`). Diff it against those rules, fix, re-run. Not closed in one attempt → stop at the base (nothing merges) |
| F3 | The XmlPort fix merges before 11b | R-RESIDUAL is re-read from the new base; C's gate asserts that set minus the 5; §3.4's SARIF set is re-measured on the new base with the same prototype before C starts |
| F4 | A's identity run fails (a golden, a `--lib` outcome outside §5.9, CDO SARIF in either universe, the histogram) | A compat projection is not today's rule. Bisect A's commits to the store, fix that projection, re-run. Not closed in one attempt → stop at C's head (C alone may merge: its gate passed) |
| F5 | B's identity run moves anything on CDO or in a fixture | §3.5's prediction is wrong for some point. Triage each moved result to a §3.2 point and a collided id; a move that follows from the union on a collided id is admitted and listed (the `golden-diff-triager` agent for a golden, `cdo-measure` + `triage-findings` for CDO); any move not on a collided id is a defect. Not closed → stop at A's head |
| F6 | The plan's pre-Task-A read of the untraced paths (§7) finds a merge point a class cannot hold | It joins the nearest class, with its rule stated in §3.2; if it needs a new class, the spec gains it before Task A (a recorded change, no new round unless it moves output) |
| F7 | A census bucket grows more than 20 % at any stage | Name it; if it is a per-row store held beside its projection, make the projection move the values; re-run the census once. Still > 20 % → reported in the numbers file and raised to the final panel (not a block: 11b has no peak bound) |
| F8 | Re-keying a point forces edits in more than 40 reader files | Re-key the producer only; the readers stay on the projection (11B-3); state it in §3.2 |
| F9 | The panel wants a per-row fan-out for edges into twin nodes (Q1) | Not 11b: 10 routes on CDO (P10), handed to step 6 (the call-site ledger); D2 stays |
| F10 | (measured: true) No natural collided row differs in facts or coverage, so B has no CDO witness | Recorded as a stated limit: T1, T1b-T1f, T2, T6 and T7 are the only evidence of the rule |

## 11. Risks and open questions for the panel

1. **Q1. An edge into a node whose rows have different ids**: one row (D2, today's anchor row),
   or one edge per row? A per-row fan-out adds 10 edges on CDO (P10).
2. **Q2. Coverage order**: "least complete wins" (P8: complete < partial < unknown). Is that
   what d35, d45, the policy gate and spans expect from a collided id?
3. **Q3. No model tag on `RoutineIx`** (§7)?
4. **Q4. Decision 25** ("cells key by string id until step 11"): under 11B-3 the `View` cells
   still key by string id after 11b. Does the epic accept "until the step that re-keys that
   reader"?
5. **Q5. `dep_routine_ids`**: a per-ix bit now, or step 8's `Role`? Proposed: step 8.
6. **Q6. The single-valued pick**: LAST (today's majority, D6) or FIRST? Or should collided
   call-site ids be disambiguated (an id format change, a non-goal)?
7. **Q7. Scope.** Stage A touches about 22 producer sites in `combined_graph.rs`,
   `detector_context.rs`, `capability_cone.rs`, `summary_runner.rs`, `events.rs`,
   `transaction_spans.rs`, `event_param_temp.rs`, `reverse_call_graph.rs`, `snapshot.rs`,
   `program_calls.rs` and the `l4` interner. B's rule has no natural witness (F10). Should the
   panel split A + B from C (F1's shape, but by choice), or keep one claim?
8. **Risk: a node-equality caller check is only as right as `node_of`.** `node_of` comes from
   the physical row (workspace) and from `source_routine_node_id` (dependency source). A wrong
   value would now pair silently where the anchor check used to fail. T4 pins the check; the
   loss gates in both universes catch an error across the population.
9. **Risk: a merge point P6 did not trace** (§7, F6).
