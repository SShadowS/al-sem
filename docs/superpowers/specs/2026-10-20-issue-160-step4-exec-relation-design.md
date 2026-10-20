# #160 child spec: step 4 — `ExecRelation`

Status: as delivered (2026-10-20), converged at spec panel round 3; the epic took
the amendments M1-M9. Where delivery differed from the design text, a dated
"As delivered" note says so in place (§4.1 M2, §4.8, §4.9, §5.5). Earlier: revised
after spec panel round 1 (`.agent/runs/20261020-034103-38ab8c/spec-r1-dispositions.json`,
items S1-1 to S1-18).
Child of `2026-10-18-issue-160-analysis-session-design.md` (the epic): §5.5, §9 row
**4**, E1, E2, E14, E17. Follows the structure of
`2026-10-18-issue-160-steps-0-2-design.md` ("child 0-2" below). Where this document and
the epic disagree, this document is wrong, EXCEPT the numbered amendments of §9 (M1-M9),
each with its reason; until the epic takes them, they are open questions for the panel.

Base: master `c554b704` (step 3 merged), worktree `U:/Git/al-sem-issue-160-a7`. Every
`file:line` is in that tree and was read before it was cited. Every fact the design rests
on is a row of the ledger's `## Assumption probes` table (`.agent/issue-160/ledger.md`,
P1-P38).

Words: **relation** = the `ExecRelation` value of a view. **Primary payload** = the
caller-to-callee edges that the solver, the reverse graph, the spans, the temp proof and
the call-edge detectors read (today's `CombinedGraph::edges_by_from`). **Cone payload** =
the typed edges the capability cone reads (today's `CombinedGraph::typed_edges`).
**Half** = the call edges of primary callers ("primary half") or of dependency callers
("dependency half"). **Legacy cut** = the graph the snapshot builds for itself from the
model's cut calls (`build_r3a3_source_only_base`, `capability_cone.rs:4216-4224`); step 6
deletes it. It is a name in doc comments, not a type (M9).

## 1. Goal

One constructor, `ExecRelation::new`, builds the execution relation of a view, in both
universes, from the two halves of the model's calls and the event graph. Everything that
today reads the cross-app base's graph, the base's SCC, or the graph the single-app view
builds (`context_graph`, `detector_context.rs:1110-1124`) reads the relation instead: the
base's leaf/fixed-role split, the detector artifacts builder and its `l4` solve,
`build_l4`, r3a5, `alsem query`, the policy RAW build. The relation keeps the dependency
half's uncertainty edges, which today are built and dropped (P9). No output byte moves
(epic I2).

What step 4 buys is structural: one owner and one recipe for "which edges carry what",
so step 5 (fold over every caller's uncertainty) and step 6 (the snapshot on the Cone
payload) change one constructor's readers, not three graph builders. It is not a speed
step (§2).

## 2. Non-goals

- **No speed bound and no cross-app peak bound.** The relation still runs two internal
  half-builds cross-app (P8); the `build.combined_graph` counter drops because the
  relation counts once, not because less is built. Epic §5.12 lists no peak bound for
  step 4; cross-app peaks are reported only (§7 item 4). **Step 4 does carry three
  single-app peak bounds** (S2-2): `analyze --single-app`, `policy check --single-app`
  and `events fanout --single-app`, the paths whose SCC lifetime changes (§4.4), each
  held under epic I6 and §5.12 (§7 item 4).
- **The snapshot stays on the legacy cut** (epic row 4; M9). `compose_snapshot`,
  `fill_ordering`'s snapshot, `project_r3a3` and the `aldump` projections keep building
  their graph with `build_combined_graph` over the model's calls (P20). Step 6 moves the
  snapshot to the Cone payload and deletes the legacy cut.
- **No fold change.** §6.3 (one coverage-fold policy over every caller) is step 5. The
  kept dependency uncertainty edges have no reader at step 4.
- **No `Role`, no `RoutineIx`, no `dependency_calls`, no `events` field** (M5-M8).
- **The `build_detector_context[_with]` shims stay** (child 0-2 §10 item 4). They take a
  relation instead of `context_graph`; deleting them touches 42 call sites in 13 files
  (P26) and nothing in step 4 needs it. Handed to step 6, whose snapshot rework rewrites
  the `aldump` context modes anyway.

## 3. Background: where each graph is built today

| What | Where | Payloads | Readers |
|---|---|---|---|
| Cross-app primary graph | `build_cross_app_base`, `capability_cone.rs:3585` over the cut calls (`:3571-3582`, primary callers only) and the FULL event graph (`:3583`) | `edges_by_from`, `edges_from_order`, `uncertainty_edges`, `nodes`, primary `typed_edges` | leaf/fixed split (`:3638-3641`), `combined_scc` (`:3588`), the context (`detector_context.rs:1114-1116`), `build_l4` (`session.rs:295-298`, `:322-349`), r3a5 (`:3825-3855`), query (`session.rs:451-460`) |
| Cross-app dependency half | `:3598-3617`: a second `build_combined_graph` over the dependency callers' edges with NO events; only its `typed_edges` are kept, appended to the primary graph's (`:3615-3617`) | dependency `typed_edges` | the cone, through the appended `typed_edges` |
| Cross-app SCC | `:3588`, `graph_scc` over `edges_by_from` (`analysis.rs:141-152`) | — | `build_l4` by pointer (`detector_context.rs:1090`, `session.rs:295-298`), r3a5 (`:3836`) |
| Single-app graph | `context_graph`, `detector_context.rs:1117-1123`, filled by `View::prepare` (`session.rs:311-315`) | all five fields over every call | the context, `build_l4`, query |
| Single-app SCC | inside `build_l4`, `analysis.rs:61-66`, AFTER the det fill's cone (`detector_context.rs:1230` then `:1493`) | — | `L4Analysis.scc`, its only owner |
| The snapshot's graph (legacy cut) | `build_r3a3_source_only_base`, `capability_cone.rs:4221`, over `resolved.calls` (cross-app: the cut) | own | the snapshot's cone and scan (`:4258`, `:4281`) |
| The cone's typed graph and its SCC | inside `compose_cone_over_graph`, `:2832`, `:2842-2846` | reads `typed_edges` ONLY (`:1233`, P13) | the cone |

Step 2 already removed the context's own cross-app graph (child 0-2 §4.4.8): the epic's
"replaces the context's rebuild (`detector_context.rs:935-945`)" is stale. What is left
is the single-app arm of `context_graph` (P3).

## 4. Design

### 4.1 The type (`src/engine/l4/exec_relation.rs`)

```rust
/// One view's execution relation (#160 step 4, epic §5.5). The two transfer
/// rules are the two accessors; the field table of §4.3 says which consumer
/// reads which.
pub struct ExecRelation {
    /// Primary payload in `edges_by_from` / `edges_from_order` /
    /// `uncertainty_edges` / `nodes`; Cone payload in `typed_edges` (primary
    /// half with event dispatch, then the dependency half, today's order).
    pub graph: Arc<CombinedGraph>,
    /// Tarjan over the Primary payload, computed in `new`.
    pub scc: Arc<SccResult>,
    /// The dependency half's uncertainty edges, in the order the half-build
    /// sorts them. No reader until step 5.
    pub dependency_uncertainty: Vec<UncertaintyEdge>,
}

impl ExecRelation {
    pub fn new(
        ws: &ModelEntities,
        primary: &[CallEdge],
        dependency: &[CallEdge],
        events: &EventGraph,
    ) -> ExecRelation;
    /// `Transfer::Cone`: every typed edge, every caller.
    pub fn cone(&self) -> &[TypedEdge] { &self.graph.typed_edges }
    /// `Transfer::Primary`: primary callers' call edges plus event dispatch
    /// from any publisher.
    pub fn primary(&self) -> &Arc<CombinedGraph> { &self.graph }
}
```

Why `CombinedGraph` stays the carrier, not separate `typed`/`combined`/`from_order`
vectors as the epic's sketch has them (M1): 32 `src` files read one of its fields through
a `graph.` value (`rg -l 'graph\.(edges_by_from|typed_edges|edges_from_order|uncertainty_edges|nodes)' src`),
13 detectors read `graph.edges_by_from` as a map, 12 files (mostly tests) build
`CombinedGraph` literals, and `DetectorArtifacts.graph` is an `Arc<CombinedGraph>`
(P14). Changing the container is a large byte-risky diff with no step-4 reader that
needs it. Step 11 changes every key to `RoutineIx` and is the step to change the
container.

Why the transfer rules are two accessors, not an `enum Transfer` value (M2): no consumer
chooses its rule at run time; each consumer's rule is fixed (§4.3), so an enum would be
matched in one place per rule. The type guarantee is ONE-directional:
`compose_cone_over_graph` takes `&[TypedEdge]` (§4.5), so **the cone cannot read the
Primary payload**. The reverse is a naming convention until step 11 splits the container:
`primary()` returns the whole carrier, typed edges included, and d43 legitimately reads
the Cone payload through `ctx.graph.typed_edges` (`d43.rs:85`).

As delivered (2026-10-20, final panel F1-1/F1-2): the fields are PRIVATE, so every
reader names its rule through `cone()`, `primary()`, `scc()` or
`dependency_uncertainty()`; `ExecRelation::single_app(model)` builds the single-app
relation (`View::prepare`, the shim, tests); and `new` prints no census: the owner calls
`emit_census()` (cross-app the base's census, single-app `single_app`).

### 4.2 Construction (byte identity)

`new` is today's cross-app recipe moved into one function, plus the dependency
uncertainty kept:

1. `pt::counter_delta("build.combined_graph", 1)` once, at entry; one span `rel.build`
   around the body (§4.8).
2. `let mut g = assemble(ws, primary, events)`: the body of today's
   `build_combined_graph` (`combined_graph.rs:259-430`) moved, unedited, into an
   uncounted `pub(crate) fn assemble(ws, edges: &[CallEdge], events)`. It reads only the
   call edges and the event graph (P10), so taking `&[CallEdge]` changes nothing;
   `build_typed_edges` takes `&[CallEdge]` likewise. `build_combined_graph(ws, resolved,
   events)` becomes the counter plus `assemble(ws, &resolved.edges, events)`; every
   legacy-cut caller is unchanged.
3. If `dependency` is non-empty: `let d = assemble(ws, dependency, &EventGraph::default())`
   (an empty event graph, as today's `no_events`, `:3609-3612`); then
   `g.typed_edges.extend(d.typed_edges)` (today's `:3615-3617`, same order) and
   `dependency_uncertainty = d.uncertainty_edges`. The rest of `d` is dropped, as today.
   Single-app passes `&[]` and skips the half-build; an empty half contributes no typed
   edge, so the skip is byte-neutral (P11). Every dedupe set in `assemble` is per call
   site and a call site belongs to one `from`, so the halves cannot interact (P32).
4. `scc = graph_scc(&g)` (`analysis.rs:141`, counter `build.graph_scc` there).
   `graph_scc` reads only `nodes` and `edges_by_from`, so taking it after the extend
   (today it is taken before, `:3588` vs `:3617`) is the same SCC (P16).
5. Census, when on: `rel.graph`, `rel.scc`, `rel.dependency_uncertainty` (§4.8).

The split stays where the cut is made. `build_cross_app_base` partitions
`x.resolved.calls.edges` by `dep_routine_ids.contains(&e.from)` (today `:3572-3582` and
`:3598-3608`) into two `Vec<CallEdge>` in emission order; it calls
`ExecRelation::new(ws, &primary, &dependency, &x.resolved.events.graph)`, then wraps
`primary` into the cut model's `ResolvedCalls` with the upgraded bindings, diagnostics
and external targets (today `:3572-3582`, `:3665`). The dependency half no longer clones
`upgraded_bindings` and `external_targets` (`:3604-3607`): the build never read them
(P10), so a transient copy of the upgraded bindings (15.5 MB on CDO, P12) is gone.

| Field | Today | Step 4 | Same because |
|---|---|---|---|
| `nodes` | `:3585`'s build | `assemble` #1 | same call, same `ws` |
| `edges_by_from`, `edges_from_order` | `:3585` over the cut calls + full events | `assemble` #1 over the primary half + full events | same edges in the same order; event loop unchanged |
| `uncertainty_edges` | `:3585` (primary half) | `assemble` #1 | same; the dependency half's go to `dependency_uncertainty`, never here |
| `typed_edges` | `:3585` then `.extend(:3617)` | `assemble` #1 then `.extend(assemble #2)` | same two builds, same order (Decision 43, I7) |
| SCC | `:3588` | step 4 of `new` | P16 |
| `dependency_uncertainty` | built at `:3617`, dropped | kept | new field, no reader |

### 4.3 The field contract (epic §5.5, reproduced)

| `DetectorContext` field | `Transfer` | Step 4 source |
|---|---|---|
| `graph.typed_edges` (d43; the cone) | `Cone` | `rel.cone()`; `DetectorArtifacts.graph` is `Arc::clone(&rel.graph)`, so d43's `ctx.graph.typed_edges` (`d43.rs:85`) is the Cone payload, as today cross-app |
| `graph.edges_by_from` (d1 d2 d3 d7 d13 d14 d16 d17 d34 d45 d46 d48 d54; `event_flow.rs:731`); d2 reads `resolution`, `subscriber_app_id` | `Primary` as `rel.combined` by `from` | `rel.primary().edges_by_from` |
| `graph.edges_from_order` (d17, first-wins sample) | `Primary` as `rel.from_order` | `rel.primary().edges_from_order` |
| `graph.nodes` (d2 `d2.rs:254`, d7 `d7.rs:144`, the reverse graph `reverse_call_graph.rs:38`, `graph_scc` `analysis.rs:149`) | none (routines) | `assemble` #1's sorted routine ids; identical in both universes |
| `reverse_call_graph`, `closed_world_temp_params`, `transaction_spans` | `Primary` | built in `build_detector_artifacts` from `rel.graph` (`detector_context.rs:1382`, `:1422-1451`) |
| `uncertainty_edges_by_from`, `uncertainties_by_node`, `resolved_call_edge_by_callsite` | `Primary` (§6.3 widens the FOLD only) | `rel.graph.uncertainty_edges` (`:1469`), the `l4` core harvest, and `model.calls` (`:1169`, `:1462-1463`), which cross-app is the primary half (P5) |
| `db_effect_bundle`, v2 solve, leaves and fixed roles | `Primary` | `build_l4(model, rel, ..)`, called by `build_detector_artifacts` with the relation it was given (§4.4); leaves and fixed roles from `rel.graph.edges_by_from` in the base (`:3638-3641`) |
| `entry_points`, `reachable_roots` | none (routines + roles) | unchanged (`:1384-1416`) |
| `event_flow_indexes`, `cross_extension_subscribers` | `rel.events` | the model's event graph, the one `new` reads; the relation holds no copy (M6) |

`Model.calls` in a cross-app view is the primary half (today's cut, `:3665`). In
`PrimaryOnly` the dependency half is empty and both rules give today's single-app
adjacency (§5.1).

### 4.4 Where it lives

- **Cross-app:** `R3a5CrossAppBase.rel: Arc<ExecRelation>` replaces `graph`,
  `combined_scc`, `event_graph` and `nodes` (§4.5). The base builds it first, because
  the leaf/fixed split reads its Primary payload.
- **The view:** the `graph` cell becomes `rel: Option<Arc<ExecRelation>>`, and
  `Cell::Graph` becomes `Cell::Rel`. `prepare` fills it where it fills `graph` today
  (`session.rs:311-315`): cross-app `Arc::clone(&base.rel)`, single-app
  `Arc::new(ExecRelation::new(ws, &model.calls.edges, &[], &model.events.graph))`.
  It stays a cell, not the epic §5.9 sketch's eager `pub rel`, because `diff` without
  `--strict` builds a `View` and fills nothing (`diff/cli.rs:122-133`, P21): an eager
  relation would add a graph build and a Tarjan to each side. No lazy fill: `prepare`
  fills it on `&mut self`; the SCC is computed inside `new` (I3).
  `session::no_lazy_fill_in_this_file` stays green. `View::rel()` replaces
  `View::graph()`.
- **`build_detector_artifacts(model, demanded, rel: &Arc<ExecRelation>, l4:
  Option<L4Request>, cross)`:** the `graph: Arc<CombinedGraph>` parameter becomes the
  relation (S1-1). It sets `DetectorArtifacts.graph = Arc::clone(&rel.graph)` (the field
  stays, P14), runs its cone over `rel.cone()`, then calls `build_l4(model, rel, req,
  cross)`, keeping today's order: cone first, then the solve, then the harvest (epic
  §5.9 rule 3 item 7). `View::prepare`, the shims and policy's RAW build pass the
  relation.
- **`build_l4(model, rel: &ExecRelation, req, cross)`:** `L4Request.scc` is deleted
  (`L4Request { extra_diagnostics }` remains); `L4Analysis.scc = Arc::clone(&rel.scc)`;
  the solve and `emit_scc_stats` read `rel.graph`. `build_l4` runs no Tarjan, in either
  universe (P15). `census_l4`'s `computed_scc` parameter and its `l4.scc` branch
  (`analysis.rs:236-242`) are deleted: always false after step 4.
- **Query:** `into_query_parts` returns `(Model, Arc<CombinedGraph>, L4Analysis)` with
  `Arc::clone(&rel.graph)`; dropping the view and the base drops every other relation
  handle, so `cross_app_query_drops_the_base` keeps asserting `(1, 1)` (P22).
- **r3a5:** `project_r3a5_cross_app` reads `base.rel.graph`, `base.rel.scc`,
  `base.resolved.events.graph` and node ids built from `base.ws_routines()`; its cone
  call takes `base.rel.cone()`.
- **Policy's RAW build** passes `view.rel()` (`pipeline.rs:265-271`) and releases
  `Cell::Rel` where it releases `Cell::Graph` (`:242`, `:309`).
- **`aldump --issue160-full-graph-timings`** passes `(*base.rel.graph).clone()`
  (`capability_cone.rs:4348`).

**The single-app SCC lives longer (S1-4).** Today it is built inside `build_l4`, after
the det fill's cone, and `l4` is its only owner, so `release(L4)` frees it. After step 4
it is built in `new`, before the cone, and the relation co-owns it. New overlaps, all
single-app (cross-app already holds the base's SCC from the base build to the view's
end):
- through the det fill's cone walk (every single-app command that fills `det`);
- `events --single-app`: from `release(L4)` (`events.rs:381`) to `release_all_but(&[])`
  after the report (`:391`);
- `policy check --single-app`: through the RAW build's cone, from `release_all_but(&[Cell::Rel])`
  (`pipeline.rs:242`) to `release(Cell::Rel)` (`:309`).

Size: the single-app SCC is 794 455 B on CDO (`l4.scc`,
`docs/measurements/2026-10-19-issue-160-step2.md:85`), about 0.9 % of the 3 % peak band
(85 MB). Splitting the SCC out of the relation into its own cell would avoid the overlap
but give the relation two owners of one fact; this spec accepts the overlap and BOUNDS
it (three single-app peak bounds, §2, §7 item 4: `analyze --single-app`, `policy check --single-app`, `events fanout
--single-app`). If a measurement shows a peak above parent + band, the fallback is the
separate cell.

### 4.5 What is deleted or changed mechanically

- `context_graph` (`detector_context.rs:1103-1124`) and its span
  `context.event_combined_graph`.
- The base's second `build_combined_graph` call and the `typed_edges.extend`
  (`:3598-3617`), the cut's inline graph build (`:3585`) and SCC (`:3588`): inside `new`
  now.
- `R3a5CrossAppBase.{graph, combined_scc, event_graph, nodes}`. `event_graph` was a clone
  of `resolved.events.graph` (`:3583`, `:3675`); `nodes` was `ws.routines`' ids in row
  order (`:3587`). Their only readers are r3a5 and the census (P18, P19); r3a5 reads the
  model's own event graph and builds the node list locally (the cone sorts it,
  `:1256-1257`).
- `L4Request.scc`; `build_l4`'s Tarjan and its span `context.core_scc_tarjan`
  (`analysis.rs:61-66`); `census_l4`'s `computed_scc` (§4.4).
- `compose_cone_over_graph(graph: &CombinedGraph, ..)` becomes `(typed: &[TypedEdge], ..)`
  and `build_typed_edge_graph(graph, nodes)` becomes `(typed: &[TypedEdge], nodes)`.
  Call sites, each one argument: the cone at `detector_context.rs:1230`,
  `capability_cone.rs:3847` (r3a5), `:4281` (legacy cut), 10 tests in `cone_derived.rs`
  (`:1070` … `:1646`); `build_typed_edge_graph` at `:2832` (the cone), `:4033`
  (`project_r3a3`) and `:4404` (`compute_r3a3_real_matrix`) (P13, S1-11).
- Docs that name what step 4 removes, re-stated in the same commit:
  `session.rs:4`, `:28`, `:312-313`; `detector_context.rs:1060-1062`, `:1107-1109`;
  `capability_cone.rs:3442-3448`, `:3454`, `:5045-5047`; `analysis.rs:26-27`; and the
  recorded discrimination of `cross_app_commit_span_sees_dependency_writes`
  (`tests/cli/cli_digest_query_dependencies.rs:466-474`), whose live seam becomes
  `ExecRelation::new`'s dependency typed-edge extend. The implementer re-runs that break
  (skip the extend in `new`) and records the outcome in the test's doc (§5.5).

### 4.6 The snapshot stays on the legacy cut

No behaviour change in `build_r3a3_source_only_base[_over]`, `compose_snapshot*`,
`fill_ordering` or `project_r3a3`. The only edits are the one-argument signature
adaptations of §4.5 (`:4033`, `:4281`, `:4404`) and one doc line naming the legacy cut
and step 6 as its deletion (M9). The cut stays observable: the snapshot builds over
`resolved.calls`, which cross-app is the primary half only, without the dependency typed
edges (#161's part 1, epic §4).

### 4.7 The epic sketch's fields, one by one

| Epic §5.5 field | Step 4 | Amendment |
|---|---|---|
| `typed` | `rel.graph.typed_edges` | M1 |
| `combined` | `rel.graph.edges_by_from` | M1 |
| `from_order` | `rel.graph.edges_from_order` | M1 |
| `uncertainty` (every caller) | `rel.graph.uncertainty_edges` (primary half) + `rel.dependency_uncertainty` | M5 |
| `events: Arc<EventGraph>`, `upgraded_bindings`, `external_targets` | not held | M6 |
| `role: Vec<Role>` | not held | M7 |
| `dependency_calls` | not held | M8 |
| `adj: [Adjacency; 2], scc: [SccResult; 2]` eager | Primary SCC eager in `new`; the Cone adjacency and SCC stay inside the cone | M3 |

### 4.8 Trace, census and stderr changes (none in output)

- **stdout, exit codes, files:** none. **stderr:** none (the non-convergence line is still
  printed by `build_l4` alone, `analysis.rs:121-123`).
- **Spans:** `rel.build` (new, inside `new`, both universes). Deleted:
  `context.event_combined_graph`, `context.core_scc_tarjan`. `crossapp.base_graph` (today
  the primary graph, SCC and field index) and `crossapp.base_artifacts_facts` (today it
  also held the dependency half-build) both change contents, so by the step 0 rule ("a
  changed span gets a new name", child 0-2 §4.1.1) they become `crossapp.field_index`
  and `crossapp.base_facts`. No test or script reads the old names (P27).
- **Census buckets:** `rel.graph`, `rel.scc`, `rel.dependency_uncertainty`, printed once
  per relation in both universes (as delivered, final panel F1-9: by the owner, not by
  `new`: cross-app with the base's census after `census.base`, single-app by
  `ExecRelation::single_app`, where `base.graph` / `context.graph` printed). Gone: `base.graph`, `base.combined_scc`,
  `base.event_graph`, `base.nodes`, single-app `context.graph`, `l4.scc`.
- **`view.release` instant:** `"cell": "Rel"` where it was `"Graph"`.

### 4.9 Counts after step 4 (what A4.3 pins)

Restated per epic I1 from child 0-2 §4.4.8's "after step 3" table. Only `bcg` moves, only
in rows that build a cross-app base: the base built two counted graphs, the relation
counts one. `scc` moves in no row: cross-app the base's Tarjan moves into `new`;
single-app `build_l4`'s Tarjan moves into `new`, and every single-app row that
constructs a relation also fills `l4` (each has `v2` = 1; P28).

| Command | program | bcg | cone | v2 | scc | pd_solve | ctx | snap |
|---|--:|--:|--:|--:|--:|--:|--:|--:|
| analyze, cross-app, default | 1 | **1** | 1 | 1 | 2 | 1 | 1 | 0 |
| analyze, cross-app, `--detector d47-…` | 1 | **2** | 1 | 0 | 2 | 1 | 1 | 1 |
| analyze `--single-app` | 1 | 1 | 1 | 1 | 2 | 1 | 1 | 0 |
| digest, cross-app | 1 | **2** | 2 | 1 | 3 | 2 | 1 | 1 |
| digest `--single-app` | 1 | 2 | 2 | 1 | 3 | 2 | 1 | 1 |
| prove (single-app until step 9) | 1 | 2 | 2 | 1 | 3 | 2 | 1 | 1 |
| fingerprint (all modes; single-app until step 9) | 1 | 2 | 2 | 1 | 3 | 2 | 1 | 1 |
| diff ws ws, per side (`--strict`: ctx 1, bcg 2, cone 2, v2 1, scc 3, pd 2) | 1 | 1 | 1 | 0 | 1 | 1 | 0 | 1 |
| events, cross-app / single-app | 1 | **1** / 1 | 1 | 1 | 2 | 1 | 1 | 0 |
| policy check, cross-app / single-app, policy loaded | 1 | **1** / 1 | 2 | 1 | 3 | 2 | 2 | 0 |
| query, cross-app / single-app | 1 | **1** / 1 | 0 | 1 | 1 | 0 | 0 | 0 |
| graph | 1 | 0 | 0 | 0 | 0 | 0 | 0 | 0 |

**The rule (M4):** `bcg` = one per constructed relation + one per legacy-cut graph (the
snapshot's, the ordering cell's snapshot's); `scc` = one per constructed relation + one per
cone composition (the det cone, policy's RAW cone, each snapshot's cone). Which commands
construct a relation (S2-4):
- **cross-app:** exactly one, inside `build_cross_app_base`, when the target is built,
  whether or not `prepare` later fills any cell (the view's `Rel` cell only clones the
  base's `Arc`). So every command whose target is the cross-app base counts it: analyze,
  digest, events, policy check and query cross-app, including a cross-app run that exits
  after the target build and before `prepare` (e.g. a digest input error), and the
  `aldump` modes that build a base (`--r3a5-cross-app-summary`,
  `--issue160-full-graph-timings`);
- **single-app:** one when `prepare` fills `det` or `l4` (one per view; `diff --strict`
  one per side). `diff` without `--strict` constructs none (its `bcg` and `scc` are the
  snapshot's), a single-app command that exits before `prepare` constructs none, and
  `graph` none.

Epic row 4's "`graph_scc` 2 per view" is therefore not a general rule: query counts 1,
policy 3, `diff` without `--strict` 1 per side.

Not in the table, stated: a single-app plan that fills the relation but no `l4` (e.g.
`analyze --single-app --detector d47-…`) now runs one Tarjan more (the relation's).
As delivered (2026-10-20, task review t45 #4): `cli_build_counters::analyze_single_app_d47`
pins that row (`bcg` 2, `scc` 2), and `analyze_cross_app_with_a_dependency_half` pins
the cross-app default row on a workspace whose dependency half is not empty (every row
above runs on a fixture whose dependency makes no call, so its half is empty); both are
in child 0-2 §4.4.8's "After step 4" table.

## 5. Test plan

Every new test lists its break, the failing assertion and the revert (I9); the proofs are
recorded in the ledger's discrimination table. Fixture preconditions read IMMUTABLE
inputs (the model's rows, its event graph, the call edges handed to `new`), never the
relation or a consumer output, so every discrimination reaches the production assertion
it is meant to fail (S1-2).

### 5.1 Per-field parity contract (`exec_relation::tests`, `--lib`)

**The oracle and its independence (S1-3).** `legacy_relation(ws, all_calls, events,
dep_ids) -> (CombinedGraph, SccResult, Vec<UncertaintyEdge>)` is today's `:3571-3617`
plus `:3588`, `#[cfg(test)]`, returning the dependency half-build's `uncertainty_edges`
it drops today. It calls `build_combined_graph`, which after §4.2 delegates to the same
`assemble` as `new`, so on the final tree it guards the composition (split, order,
events, halves apart), not `assemble` itself. Independence of the extraction is proven
at the commit where it matters, in two commits:

1. **Commit A:** add `exec_relation.rs` with `assemble` as a NEW function (the body of
   `build_combined_graph` and `build_typed_edges` copied, `build_combined_graph`
   untouched), `ExecRelation::new` over it, and the oracle over the untouched
   `build_combined_graph`. Run the parity test on the corpus and on CDO (§5.1's CDO
   half): old builder vs new assembler, independent code. Assembler-internal
   discrimination on this commit: in `assemble`, sort each `edges_by_from` list by `to`
   instead of `edge_sort_key` → `edges_by_from` fails; set every `DispatchMeta.isolated`
   to `false` → `edges_by_from` fails on `tests/r0-corpus/ws-txn-d47-event-neg-isolated`
   (S2-1). That dir has an isolated publisher WITH a subscriber:
   `[IntegrationEvent(false, false, true)] OnAfterProcessIsolated`
   (`src/Publisher.Codeunit.al`) and `HandleAfterProcessIsolated` subscribing to it
   (`src/Subscriber.Codeunit.al`); `parse_isolated` reads the third argument
   (`src/program/model/events.rs:120-137`) and the dispatch edge copies it
   (`combined_graph.rs:392-395`). Precondition, asserted from the immutable event graph
   of that dir's model: an `EventSymbol` with `isolated == Some(true)` AND at least one
   `EventEdge` whose `event_id` is that symbol's id. `ws-event-isolated` does not count
   (no subscriber), and the §5.2 fixture's event is not isolated
   (`[IntegrationEvent(false, false)]`), so neither is a fallback: if the named dir stops
   meeting the precondition the test fails, it does not skip. Both reverted, green.
2. **Commit B:** make `build_combined_graph` call `assemble` and delete the copied body.
   Before deleting, the implementer checks the two bodies are identical text apart from
   the parameter (`&resolved.edges` vs `edges`), and records that check in the ledger.

On the final tree the oracle stays as the composition guard until step 6 deletes the
legacy cut.

**`relation_equals_the_legacy_base_per_field`:** for every app dir at any depth under
`tests/r0-corpus`, `tests/fixtures`, `tests/r3a4-fixtures`, `tests/r3a5-fixtures` and
`tests/r2-5b-fixtures` (step 3's set), plus the inline fixture of §5.2:
- single-app model: `legacy_relation(.., dep_ids = ∅)` against `new(ws, &calls.edges, &[], ..)`.
  The single-app path never cuts today (`detector_context.rs:1118`), even when the model
  carries dependency ABI rows, so the oracle's set is empty, not derived from app guids;
- cross-app model: `dep_ids` = the base's `dep_routine_ids`.

Fields, each asserted separately so a break names its field: `nodes`, `edges_by_from`,
`edges_from_order`, `uncertainty_edges`, `typed_edges` (as `Vec`, so order counts), the
SCC (`SccResult: PartialEq`, P16), and `dependency_uncertainty` == the legacy half's
list. A dir that builds no model fails the test (step 3 r1 fix). Preconditions, read from
the inputs: at least one case has a non-empty dependency half AND at least one
event-dispatch edge in its event graph (the inline §5.2 fixture has both, so this cannot
fail on corpus drift). Whether any corpus dir has a non-empty `dependency_uncertainty` is
printed, not required; §5.4's inline witness is the mandatory one (S1-10).

- **CDO half:** the same comparison on `U:/Git/DO-cdo-baseline/Cloud`, both models, gated
  by `crate::cdo::cdo_ws_or_enforce()` (step 3's pattern, `capability_cone.rs:5857-5862`)
  and run by `scripts/cdo-gate`, which runs the whole `--lib` (`scripts/cdo-gate:101-102`,
  P29).
- **Composition discriminations** (final tree): (a) extend the dependency typed edges
  BEFORE the primary ones → `typed_edges` fails; (b) pass `events` to the dependency
  half-build → `typed_edges` fails (duplicate dispatch typed edges, on the §5.2 case);
  (c) put the dependency edges into the primary half → `edges_by_from` fails; (d) append
  the dependency uncertainty to `uncertainty_edges` → `uncertainty_edges` fails. Each
  reverted, green.

This pins the constructor. The USE (every consumer reads the relation) is pinned by the
goldens (r3a5 over `rel.graph` and `rel.scc`; r4 cross-app; cli),
`cross_app_commit_span_sees_dependency_writes` (the cone sees the dependency typed edges
through the relation, §4.5), §5.2, §5.3, §5.4 and the counters (§5.5).

### 5.2 The dependency-publisher fixture (epic row 4; `capability_cone::tests`, `--lib`)

Placement: `capability_cone::tests`, beside the View-path precedent
`dependency_publisher_roles_stay_unknown_through_the_view` (`capability_cone.rs:5089-5130`),
because the fixture writers there are private to that module (S1-17).

Precedent (P23, falsified in round 1): `write_fixture_107(.., true)` already has a
workspace subscriber (`HandleWs`) on a source-bearing dependency publisher (`OnDepRec2`,
`:5144-5181`). What is new here is a dependency CALLER of the publisher, a `Commit` in the
subscriber, and a dependency `local` helper. A new writer `write_fixture_step4`, on
`write_fixture_107`'s pattern (`:5189-5226`):
- dependency `SymbolReference.json`: table 18 `Customer`; codeunit 12 `Dep Rec Lib` with
  methods `Post(var Rec: Record Customer)`, `OnDepPost(var Rec: Record Customer)` carrying
  the `IntegrationEvent` attribute (`[{"Name":"IntegrationEvent","Arguments":[{"Value":"False"},{"Value":"False"}]}]`),
  `CallTouch()`; `Touch` is `local`, so it is not in the symbol list;
- dependency source `src/Lib.al`: `procedure Post(var Rec) begin OnDepPost(Rec); end;`,
  `[IntegrationEvent(false, false)] procedure OnDepPost(var Rec) begin end;`,
  `procedure CallTouch() var T: Record Customer temporary; begin Touch(T); end;`,
  `local procedure Touch(var R: Record Customer) begin R.Modify(); end;`; plus
  `src/Customer.al`;
- workspace codeunit `Ws Fwd`: `procedure DoPost(var Rec) begin Lib.Post(Rec); end;`
  (named `DoPost`, not `Run`, so the root literal cannot be confused with
  `Codeunit.Run`/`OnRun`), `procedure DoTouch() begin Lib.CallTouch(); end;` (makes the
  helper demanded, since S8.2 the cross-app model holds only demanded dependency code,
  `program::resolve::demand`), and `[EventSubscriber(.., 'OnDepPost', ..)] local procedure
  HandleWs(var Rec) begin Rec.Modify(); Commit(); end;`.

Production path: `build_cross_app_base`, then `View::prepare` with
`digest_plan(default_detectors())`, then read `view.det()`.

Preconditions, from immutable inputs (asserted first):
- `OnDepPost`, `Post`, `CallTouch` and `Touch` are model routines in `dep_routine_ids`;
  `Touch` is `kind == "procedure"`, `access_modifier == Some("local")`, and its `R`
  parameter's `temp_state.kind == "parameter-dependent"`;
- the model's event graph has an `EventSymbol` whose `publisher_routine_id` is
  `OnDepPost` and an `EventEdge` for it to `HandleWs`;
- the dependency half holds `Post → OnDepPost` and `CallTouch → Touch`. Where it comes
  from (S2-3): the base keeps only the cut calls (`capability_cone.rs:3664-3665`), so the
  test builds the cross-app model a second time, independently, with the same call the
  base makes, `program_calls::assemble_and_resolve_cross_app_program(&inputs, "r0",
  false)` (`program_calls.rs:897-901`; the base calls it at `capability_cone.rs:3550-3554`),
  and splits its uncut `resolved.calls.edges` by the routines whose `app_guid` differs
  from `resolved.primary_app`'s, the base's own rule (`capability_cone.rs:3562-3569`).
  The build is deterministic (epic I7), so this is the input `new` received. A shared
  test helper `dependency_half(ws) -> Vec<CallEdge>` does this for §5.2 and §5.4.

None of these reads the relation, so a break inside `new` cannot fail them.

Hand-stated expectations (literals):
1. `reverse_call_graph[HandleWs]` = one `event-dispatch` edge, `from = OnDepPost`.
2. The `transaction_spans` entry for `HandleWs`'s `Commit` has span roots `[OnDepPost]`.
   The backward walk (`transaction_spans.rs:101-140`, which also stops at another
   committing routine; roots by `:265-271`) goes `HandleWs` → `OnDepPost` and stops:
   `Post → OnDepPost` is a dependency caller's call edge, in the Cone payload only.
3. `closed_world_temp_params` contains no entry for `Touch` (today `Touch` has no Primary
   reverse caller, so `prove` refuses it as a dead local routine, `closed_world_temp.rs:244-246`;
   and `CallTouch`'s call site is not in `resolved_callsites`, `:113-120`, `:229-238`).

The implementer runs the test body on the BASE commit before any step-4 edit and checks
(1)-(3) there; if the base disagrees with a literal, the spec's reading was wrong and the
panel is told, not the literal quietly changed.

Discriminations:
- **"Primary callers only"** (epic row 4): in `new`, drop the `event-dispatch` edges whose
  publisher is a dependency routine. Expected: (1) fails (no edge), (2) fails (root
  `HandleWs`). (3) stays green, and this limit is narrow (S1-6): an `event-dispatch` edge
  targets only an `event-subscriber`-kind routine (event edges are made only for routines
  with that kind, `src/program/model/events.rs:380-381`; the kind comes from the
  `EventSubscriber` attribute, `src/program/body/ir_walk.rs:2762-2767`),
  which `prove` rejects (`closed_world_temp.rs:206`, `kind != "procedure"`), and it is not
  a `direct`/`method` edge (`:251`). So removing dispatch edges cannot move the temp proof.
- **"Every caller"** (Primary = Cone's callers): in `new`, assemble the Primary payload
  over `primary ++ dependency`. Expected: (2) fails (the walk climbs `OnDepPost` → `Post`
  → `DoPost`, roots `[DoPost]`); (3) fails: `Touch` gains its sibling caller and a
  resolved call site, and `CallTouch` passes a `temporary` record, so `(Touch, R's index)`
  is proven; the r3a5 golden also moves (dependency routines stop being solver leaves,
  `:3636-3648`). This is what makes (3) a use pin of the Primary rule, as row 4 asks.

### 5.3 The two-caller `from_order` fixture (epic row 4; `capability_cone::tests`, `--lib`)

Inline in a tempdir, NOT a new `tests/r0-corpus` dir (a new corpus dir moves three golden
families, CLAUDE.md; P25): `ws-d17-drift`'s `app.json` and `.alpackages` copied, with two
caller codeunits in two files, `A` and `B`, each `Codeunit.Run(Codeunit::D17DepNewer)`.

Production path: `build_cross_app_base`, `View::prepare(analyze_plan(vec![d17], true))`,
read the run's findings.

Precondition, from the immutable input: in `base.resolved.calls.edges` (the primary half
handed to `new`), the first edge whose `from` is `A`'s routine comes before the first
whose `from` is `B`'s. This reads the call list, not the relation, so reversing
`edges_from_order` inside `new` leaves it true.

Expectation (literal): exactly one `d17/dddddddd-0000-0000-0000-0000000000d17` finding,
whose evidence step's `routine_id` is `A`'s routine and whose `affectedObjects` is
`[A's object]`.

Discrimination: reverse `edges_from_order` in `new` (after `assemble`). Expected: the
precondition still holds, the finding's sample flips to `B` and the expectation fails;
the ledger records the observed `B` routine id. Reverted, green.

### 5.4 Dependency uncertainty kept, never read (`capability_cone::tests`, `--lib`)

Mandatory witness. Inline fixture: a dependency `.app` with source whose routine
`DepCaller` calls a method that does not exist on a record (an unresolved or
member-not-found call), called from the workspace (so it is demanded). Precondition, from
the input: the dependency half, built by §5.2's `dependency_half` helper from an
independent cross-app model build (not from the base, which keeps only the cut calls),
holds a to-less edge `from = DepCaller`.
Through `build_cross_app_base`:
- `rel.dependency_uncertainty` holds exactly one edge, `from = DepCaller`, of the kind
  the to-less edge's resolution maps to (`combined_graph.rs:335-361`);
- no `rel.primary().uncertainty_edges` entry and no context `uncertainty_edges_by_from`
  key is `DepCaller` (through `View::prepare` with a plan holding d1, so the context is
  built).

Discriminations: break (d) of §5.1 → the second assertion fails; drop the
`dependency_uncertainty` assignment in `new` → the first fails. Reverted, green.

### 5.5 Counters and the re-stated seam

`tests/cli/cli_build_counters.rs`: the cross-app rows' `bcg` cells edited to §4.9 in the
same commit (four `Row` literals plus `EVENTS_CROSS_ROW` and `QUERY_CROSS_ROW`, eight
tests). Discriminations: (a) build the dependency half through the counted
`build_combined_graph` → `analyze cross-app` reads `bcg` 2 and fails; (b) keep a Tarjan
in `build_l4` (ignore `rel.scc`) → `analyze single-app` reads `scc` 3 and fails.

As delivered (2026-10-20): break (a) as worded stays GREEN on the `ws_dep` rows, whose
dependency makes no call, so `new` skips the half-build. It fails (`build.combined_graph:
got 2, want 1`) on the added `analyze_cross_app_with_a_dependency_half` case, whose
dependency half is not empty (task review t45 #1; the ledger records both runs).

`cross_app_commit_span_sees_dependency_writes`: skip the dependency typed-edge extend in
`new` → expected `["span-has-no-known-writes"]`; reverted, green; recorded in its doc
(§4.5).

### 5.6 Tests rewritten with the code they pin (I2's exception)

| Test | Change | Why |
|---|---|---|
| `cli_build_counters::retained_census_counts_the_single_app_scc_once` | expects one `bucket=rel.scc` line and zero `l4.scc` | the single-app SCC moved into `new` |
| `cli_build_counters::retained_census_counts_the_cross_app_graph_and_scc_on_the_base` | renamed `…_on_the_relation`; one `rel.graph` and one `rel.scc`; zero `base.graph`, `base.combined_scc`, `context.graph`, `l4.scc` | the base fields are deleted |
| `cli_build_counters::views_release_their_cells_before_the_next_build` | `rel:Graph` → `rel:Rel`; its doc's `release_all_but(&[Cell::Graph])` → `Cell::Rel` | `Cell::Rel` |
| `capability_cone::tests::cross_app_shim_reuses_the_base_scc` | `ptr_eq(&l4.scc, &base.rel.scc)` | field moved |
| `capability_cone::tests::dependency_publisher_roles_stay_unknown` | `build_l4(&base.resolved, &base.rel, L4Request { extra_diagnostics }, ..)` | signature only |
| `detector_context::tests::core_summaries_without_an_l4_request_panics` | builds an `ExecRelation` and passes it to `build_detector_artifacts` | `context_graph` deleted; new signature |
| `summary_runner` test at `:2584-2590`, the 10 `cone_derived.rs` cone calls, `session::tests` (`Cell::Graph` at `:743`) | signatures / names | mechanical |
| `cross_app_commit_span_sees_dependency_writes` | doc only (§5.5) | its recorded seam moved |

`cross_app_query_drops_the_base` keeps its assertion `(1, 1)` (P22); only its doc
(`:5045-5047`) is re-stated.

## 6. Acceptance matrix

| # | Epic row 4 item | Evidence |
|---|---|---|
| A4.1 | `Transfer::{Cone, Primary}`, two payloads | `rel.cone()` / `rel.primary()` (§4.1, M1/M2); the cone takes `&[TypedEdge]` (compile-time, one direction); §5.1 per-field parity |
| A4.2 | Eager adjacency/SCC | Primary SCC in `new` (§4.2 step 4); `build_l4` runs no Tarjan (§5.5 b). The Cone pair is deferred by M3 (a stated tradeoff, §9) |
| A4.3 | Counters | `cli_build_counters` at §4.9 under M4's rule |
| A4.4 | Base, context, r3a5, L4 read §5.5's table | §4.3 (each field's source); `build_detector_artifacts` takes the relation (§4.4); goldens; §5.2-§5.4 |
| A4.5 | Snapshot on the legacy cut | no behaviour change in the snapshot path (§4.6, M9); cli-b and r3a3 goldens unchanged |
| A4.6 | Dependency uncertainty edges kept | §5.4; `rel.dependency_uncertainty` census bucket on CDO (§7) |
| A4.7 | Byte-identical everywhere | 8 golden targets (`scripts/check-goldens`) + `cargo test -p al-sem --lib`; CDO `aldump --program-call-graph-stats` SHA unchanged |
| A4.8 | CDO detector baseline identical | `cdo-measure` before/after: no detector count change at `--scope primary` or `--scope all`, fingerprints added 0 / removed 0 (§7) |
| A4.9 | Dependency-publisher fixture, "Primary callers only" discrimination | §5.2 |
| A4.10 | Two-caller `from_order` fixture, reverse it, d17 flips | §5.3 |
| A4.11 | E14 CDO ratchets | `scripts/cdo-gate U:/Git/DO-cdo-baseline/Cloud`, incl. §5.1's CDO half |
| A4.12 | E17 Transfer rules reproduce today's adjacency per field | §4.3 + §5.1 + §5.2 |

Invariants touched: I1 (§4.9), I2 (A4.7; §5.6), I3 (§4.4: a `prepare`-filled cell, SCC
in `new`), I4 (no fill-state input), I7 (concatenation and `from_order` recorded, §4.2),
I9 (§5), I10 (A4.11), I6 (the three single-app peak bounds, §7 item 4). I5, I8, I11
untouched. Epic §5.9 rule 3's "last use drops"
holds per cell; the single-app SCC's longer life inside `Cell::Rel` is stated and
measured (§4.4).

## 7. Measurement plan

All on the pinned CDO workspace, `release-fast`, base `c554b704` against head, the
`cdo-measure` discipline (both binaries copied before running; both sides must name
`bc3ccb18` with the same dirty count, else the comparison is void).

1. **Findings:** `measure.py run` for both sides, `measure.py comp`: no detector changed,
   both scopes; fingerprints added 0, removed 0. A non-zero delta fails the step (I2).
2. **Histogram:** `aldump --program-call-graph-stats` JSON SHA-256 equal on both sides.
3. **cdo-gate:** green, with the §5.1 CDO half (run on commit A too, §5.1).
4. **Peaks.** Three single-app peak BOUNDS (S2-2) and two cross-app reports.
   - **Bounded, under epic I6 and §5.12:** `analyze --single-app`, `policy check
     --single-app` (with the owner's CDO policy, or
     `tests/fixtures/cli-c-policy/ws-policy-custom/al-sem.policy.yaml` if none) and
     `events fanout --single-app`, the paths whose SCC lifetime changes (§4.4). The
     baseline is the parent commit `c554b704`, measured in the same session (Decision
     20), interleaved with the head by `scripts/bench_ab.py run --rounds 5` (≥ 3 runs
     per side, I6). Each path has two bounds, both relative (Decisions 22, 33): head
     median process peak ≤ parent median + band, and head median post-attach peak
     (`peak.post_attach_mb`) ≤ parent median + band, where band = §5.12's peak noise band,
     3 % of that path's own parent median (the "about 85 MB" of §5.12 is 3 % of CDO's
     cross-app peak; the single-app paths' bands are smaller and are written into the
     numbers file from the parent runs before the head is measured). stdout hash
     equal on both sides. A miss fails the step and walks §5.12's ladder; its first rung
     here is the separate-SCC-cell fallback of §4.4, since the expected change is ~0.8 MB.
   - **Reported, no bound** (the epic sets none for step 4): `analyze` and `digest`
     cross-app, both peaks, same harness and rounds. Expected direction, not a claim:
     lower by the base's deleted event-graph clone and the dependency half's deleted
     transient `upgraded_bindings` clone, higher by `rel.dependency_uncertainty`.
5. **Census:** one `ALSEM_RETAINED_CENSUS=1` analyze per side and universe; the numbers
   file lists every bucket that changed (§4.8) with its bytes.

Numbers go to `docs/measurements/2026-10-20-issue-160-step4.md`.

## 8. False-positive and failure shapes

- **Concatenation order flips** (dependency typed edges first): d43's dispatch-site
  order and the cone's raw tie-breaks could move. Caught by §5.1 (a) and r3a5/cli goldens.
- **Dependency uncertainty leaks into Primary:** more uncertainty in d1-family witnesses
  and the L4 solve before step 5 decides it. Caught by §5.1 (d), §5.4.
- **Dependency-caller dispatch dropped** ("Primary callers only"): spans, reverse callers,
  d2's `subscriber_app_id` view change. Caught by §5.2.
- **Dependency call edges admitted to Primary:** dependency routines stop being solver
  leaves, spans climb into dependency code, dependency `local` helpers gain temp proofs.
  Caught by §5.1 (c), §5.2's second break.
- **An assembler edit hidden from the oracle:** on the final tree the oracle shares
  `assemble`; commit A's independent comparison and its two internal mutations (§5.1)
  cover the refactor itself, and goldens cover later edits.
- **A second relation per view** (a shim or a command building its own): `bcg` 2 in the
  counter rows.
- **The relation alive longer than the graph was:** the graph's lifetime is unchanged
  (`Cell::Rel` is released where `Cell::Graph` was; `views_release_their_cells_before_the_next_build`
  pins the order). The single-app SCC's longer life is §4.4's stated, measured exception.

## 9. Amendments, risks, open questions

Amendments this spec asks the epic to take (the panel decides; until then each is an open
question):

- **M1.** §5.5's struct: `CombinedGraph` remains the carrier of both payloads at step 4
  (§4.1); `typed`/`combined`/`from_order` as separate vectors move to step 11, with the
  `RoutineIx` keys.
- **M2.** `Transfer` is two accessors plus the cone's `&[TypedEdge]` parameter, not an enum
  value; the guarantee is one-directional until step 11 (§4.1).
- **M3.** The Cone adjacency and SCC stay inside the cone at step 4. This is a tradeoff,
  not a "no reader" claim. Today two consumers build the Cone pair for the same view:
  the det cone, and in `policy check` also the RAW build's cone (`pipeline.rs:265-271`,
  each `compose_cone_over_graph` builds a typed graph and runs Tarjan, `:2832-2850`), so
  policy builds the pair twice. Holding the pair in the relation would remove policy's
  second build (the typed graph plus one Tarjan; graph, Tarjan, dedup and coverage
  together are < 0.25 s on CDO, epic §3), at the cost of keeping the typed graph alive
  from the det cone through every run, policy's rule engine and, in digest and prove,
  through snapshot composition beside the snapshot's own typed graph; and of one Tarjan
  more in every view that builds no cone (query; `analyze --detector d47-…`). Policy's
  duplicate is temporary: step 6 deletes the RAW build (Decision 16), and step 6 is also
  where the snapshot becomes the second permanent reader of the Cone pair. So the pair
  joins the relation at step 6 (or 7a, whichever spec lands it first), and until then
  policy check keeps its duplicate, stated in §4.9's rule (policy `scc` 3).
- **M4.** Row 4's counter acceptance reads as §4.9's rule: one SCC per constructed
  relation plus one per cone composition; not "2 per view".
- **M5.** `uncertainty` "every caller" is two fields at step 4: the primary half stays in
  `rel.graph.uncertainty_edges`, because the context's `uncertainty_edges_by_from`,
  `uncertainties_by_node` and the L4 solve must stay Primary (§4.3, §6.3 widens only the
  fold, at step 5); the dependency half is `rel.dependency_uncertainty`. Step 5 decides
  whether one list with a role filter replaces the two.
- **M6.** No `events: Arc<EventGraph>`, `upgraded_bindings` or `external_targets` in the
  relation. The readers use `model.events.graph` and `model.calls`, the same bytes
  (`analysis.rs:86`; `detector_context.rs:1171`); the base's duplicate event-graph clone
  is deleted instead (§4.5). Sharing the det's own clone by `Arc` is a memory change for
  a later step, not a transfer rule.
- **M7.** No `role: Vec<Role>`: `Role` is step 8 and `RoutineIx` step 11; the halves split
  on `dep_routine_ids`, keyed by string id (Decision 25).
- **M8.** No `dependency_calls` at step 4, departing from Decision 18's placement in time,
  not in substance: its one reader is step 6's call-site ledger, and holding the raw
  dependency `CallEdge`s from step 4 would retain them through every run with no reader.
  Step 6's spec keeps the dependency half in `new` as `rel.dependency_calls`.
- **M9.** Row 4's "snapshot on `LegacyCut`" is a name, not a type: the snapshot path is
  unchanged (§4.6) and its doc comments call the graph the legacy cut until step 6
  deletes it. A type or enum with one variant would have one reader and no choice.

Risks:

1. **§5.2's literals** rest on this spec's reading of the span walk, the temp proof and the
   demand of dependency `local` helpers; none was run. The implementer checks them on
   the base commit first (§5.2).
2. **Single-app SCC lifetime** (§4.4): bounded by three single-app peak bounds (§7 item
   4); fallback is a separate SCC cell.
3. **Single-app d47-only plans** gain a Tarjan (§4.9), unpinned and cheap.
4. **Shims** (`build_detector_context[_with]`) survive one more step (§2).
5. **The epic's line citations** for step 4 predate steps 1-3; this spec re-located every
   one (§3, P31).
