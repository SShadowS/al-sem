# #160 child spec: step 6 — one cone graph, one scoped witness pass

Status: amendment 6A-12 added as §14 (2026-10-23, attempt 8, base `6a2b7c8b`; the operator
decided its four questions, §14.13), awaiting its spec-panel round. **§14 overrides §0-§13
where they disagree**: step 6 moves digest only, the ordering cell stays on the cut graph,
and one DAG witness rule replaces the per-fact search. Before it: spec panel round 3 fixes
applied (findings R1-01 to R1-33, R2-01,
R2-02, R3-01 and R3-02 in `.agent/issue-160/findings.json`, §0.1; round 2 re-raised R1-02,
R1-17, R1-24, R1-28). Made from the pre-draft
`/work/bench/160-6/draft-step6-design.md` (master `5c42b8f3`) once the base probes had run;
§0 lists what the probes changed, §0.1 what round 1 changed.

Child of `2026-10-18-issue-160-analysis-session-design.md` (the epic): §1 goals 2, 6; §5.5
(M3, M5, M8), §5.6, §5.7, §5.9 rule 3, §5.10, §5.12; §9 row **6**; E1, E3, E9, E12, E15, E18;
I1-I10; Decisions 2, 8, 14, 15, 16, 18, 20, 22-24, 33, 41-43, 47, 50-52, 56, 58; carried
findings G-1, G-2, G-4, R6-4; open questions 12, 13, 14. The shape follows
`2026-10-22-issue-160-step5-coverage-policy-design.md`. Where this document and the epic
disagree, this document is wrong, EXCEPT the numbered amendments of §9 (6A-1 to 6A-12). Each
gives its reason; until the epic takes it, it is an open question for the panel.

Base for the citations: master `7afa91a3` (#220's merge, #229; re-cited from `459294f1`,
step 5's merge, #223: only the files #220 edits moved), worktree
`/work/al-sem-issue-160-a7`. Every fact the design rests on is a row of the ledger's
`## Assumption probes` table (`.agent/issue-160/ledger.md`). Scripts and raw output:
`/work/bench/160-6/` (`probe-base.sh`, `probe-base2.sh`, `probe/`). Nothing from the probes is
on the branch.

**#220 (epic step 8) merges BEFORE this step's implementation, and the post-#220 master is the
parent, unconditionally** (Decision 20). #220 changes the snapshot schema (4) and the role
rule, so every base number here (bands, spans, peaks) is indicative only: Task 1 re-measures
every term on the post-#220 parent, and the bench's `P` is that commit. Citations marked
**[#220]** are in files #220 edits (`snapshot.rs`, `snapshot_full.rs`, `capability_cone.rs`
near `build_cross_app_base`, `exec_relation.rs`, `digest.rs`, `selector_index.rs`,
`registry.rs`, `gate/run.rs`, `test_support.rs`, `fingerprint_cli.rs`, `gate/diff/cli.rs`,
`program/model/role.rs`); they are re-cited at the plan. §11 lists what #220 hands over.

Words:
- **Universe** = every routine row of the view. **Subject** = a routine whose raw inherited
  facts some job reads. **`WitnessScope`** = the subject set.
- **Cut graph** = the snapshot's own `build_combined_graph` over `model.calls`
  (`capability_cone.rs:4514-4522`), which cross-app holds primary callers only ("legacy cut",
  epic M9; there is no `LegacyCut` symbol, only that doc text). **Cone graph** = `rel.cone()`
  (`exec_relation.rs:111-114`): the primary half's typed edges with event dispatch, then the
  dependency half's.
- **Raw** = a routine's `Vec<CapabilityFact>` of inherited facts. **Derived** = its
  `ConeDerivedStore` row. **pd** = the producer solve's `ConeArena`
  (`capability_cone.rs:2641`).
- **Tree** = the view's `CapabilitySnapshot` (`snapshot.rs:883`) **[#220]**. **Effects** = the
  per-root digest entries (with their unresolved cones) and the ordering facts computed from the
  tree.
- **The bound commands** (epic row 6): `d` = cross-app `digest <WS> --file
  src/DocumentTypes/Finance/General/CDOIssueDocument.Codeunit.al --format json` (3 roots at the
  base); `o` = cross-app `analyze <WS> --detector
  d47-io-unsafe-txn,d49-uncommitted-write-before-ui,d51-retry-side-effect-duplication
  --format sarif`.
- Memory units: `maxrss.py` prints `ru_maxrss` in KiB. 14,331,236 KiB = 13.67 GiB.

## 0. What the probes changed in the pre-draft

1. **A6.4's wall bound cannot hold as worded, for the shape the row asks for.** `o` builds no
   det cone today (its `cone 1` is the cut snapshot's, `cli_build_counters.rs:233`; d47/d49/d51
   require `ORDERING_FACTS` only, `detectors/mod.rs:1575`, `:1592`, `:1609`), so nothing can
   be fused: the head composes once over the full cone graph where the parent composed over the
   cut. Indicative base numbers: a full-graph composition in DerivedOnly mode is 6,136 ms
   (`context.capability_cones`, cross-app default analyze, `direct=` included; `pd_solve`
   2,788 ms, fact cones 1,543 ms); the cut's is 677 ms (`r3a3.compose_cone`; `pd_solve` 74 ms).
   The modes differ (`o`'s head composition is `{derived: false, raw: roots}`), so these
   numbers show that the head adds a large cost, not its size; Task 1 measures it in the head's
   exact shape (§8.4) and 6A-10 states the cost bound.
2. **Policy violates I5 today.** Its RAW build composes raw for EVERY routine on the cone graph,
   dependency rows included (`det_cone_inputs` nodes = every model routine,
   `detector_context.rs:1266`; `Both` at `:1375`). The base peaks at 13.67 GiB (custom policy)
   and 12.59 GiB (bundled); the two compositions together take 36.6 s of cone time, the
   derived-only one ≈ 6.1 s. Scoping it to primary subjects is the step's largest memory and
   time win.
3. **The void check is fragile on a shared server1.** At the base, `d`'s band is 2.1 s (9.8 %
   of 21.6 s); `o`'s `analyze.total` band is 2.6 s on the clean side (14 %), 12.3 s on the
   side a load spike hit. 10+11a measured `d`'s band at 0.2 s in a quiet slot. §8 re-measures
   `o`'s parent band in the Task 1 slot and pre-states the band rule before any code (R1-26).
4. **The cone is still keyed by string id after 11b** (`capability_cone.rs:2586`), so
   `WitnessScope` holds string ids.
5. **#220 delivers the gap half of Decision 8** with `rel.cone()` reach (6A-4; §11).
6. **prove, fingerprint and diff stay on a standalone composition** (6A-9).
7. P1-P4 head halves and P8 need the scope type; they are Task 1's measurements and the stop
   rule's inputs (§8.4). P5, P9, P13 hold at the base. Q3 is decided statically (§3.2).

### 0.1 What spec panel round 1 changed

- **One copy of raw** (R1-01): the composition returns raw APART from coverage (`caps.raw`),
  the tree takes it by move, and F1 (release after use) is withdrawn: it could not lower a
  peak that is reached when the composition returns. F2 (ingredient gate) is the first memory
  rung.
- **Cost terms in the head's exact shape** (R1-02, R1-03): K, T, E′ from a Task 1 probe, the
  cut base and derive credited; the memory test reads a measured process peak, not a sum of
  shallow census buckets (R1-04).
- **Cells** (R1-05, R1-06): `Cell::Caps`, `Cell::Raw`, `Cell::Tree`, `Cell::Effects`, each
  with its fill, reader and release; `needs_tree` apart from the raw scope; `identities` a
  command-local until step 12 (6A-11).
- **Digest frees its whole tree before `det`** (R1-07, was F3): the effects cell computes each
  root's unresolved cone and keeps only `identities` and `coverage`.
- **Standalone composition reuses the cut's counted builder** (R1-08): `compose_snapshot`
  (single-app only, asserted) keeps `build_combined_graph`, so prove/fingerprint/diff counters
  do not move. `compose_full_snapshot` is covered in both universes (R1-09).
- **`inherited_raw` becomes test-only** instead of deleted (R1-10): 24 fixture call sites and
  the raw oracles keep their input; production cannot carry raw.
- **D6's reader corrected, and its ordering effect stated** (R1-11); T1, T3, T4, T7, T8, T9
  rewritten so each break can fail (R1-12 to R1-17); F5 now stops instead of licensing a second
  pass (R1-18); the "no unscoped path" claim narrowed (R1-19); Q3 decided statically (R1-20).

## Feasibility first

Speed step (§5.12's peak list). Measured on server1 (22 cores, 93 GB, shared with another
session); base numbers indicative until the post-#220 re-measure.

| Item (row 6) | Verdict | Evidence |
|---|---|---|
| Unscoped tree on the cone graph | not feasible, not needed | step 0: 31.4 GB peak; the tentacle limit is 32 GB |
| One composition per view where a det cone exists (digest, policy, analyze with summaries) | feasible by construction | §4.2 |
| #161 acceptance; context == tree coverage | feasible | T1, T8 |
| **A6.3 `d` wall ≤ parent + band** | **likely**: the head deletes the cut base (952 ms) and derive (988 ms) and adds the raw walk for 3 roots, the cone-graph tree derive and the unresolved cones of 3 roots (T, Task 1) | P3 base half |
| A6.3 `d` peaks | process peak likely (3.15 GB, set in attach); the post-attach peak is not readable on Linux (#204, 6A-8) | P7 peaks |
| **A6.4 `o` wall ≤ parent + band** | **not deliverable as worded, because** the parent composes over the cut and the head must compose over the full cone graph (§0 item 1). Delivered as 6A-10's cost bound | §0 item 1 |
| A6.4 `o` peak ≤ 1.03 × parent | unknown until Task 1: what is new is the raw of 5,479 reportable roots and the cone-graph tree, at the top of the graph where cones are largest. Ceiling: every routine's raw is 9.4 GiB (step 0). F2 is the first rung | §8.4 S2 |
| counters at the final row | feasible | §4.9 |
| no raw for a dependency subject | feasible by construction; fixes policy's violation | D3, T8 |
| CDO `policy check` moves nothing | feasible: same composition, same facts for the primary routines it evaluates | P10 SHAs |
| stop rule | defined in §8.4, read after Task 1 | — |

## 1. Goal

1. **One cone graph.** The view's tree is composed over `rel.cone()`. The cut graph's builder
   (`build_r3a3_source_only_base`, `_over`, `R3a3SourceBase`, `capability_cone.rs:4485-4623`)
   and the measurement-only `aldump --issue160-full-graph-timings` (`:4625-4694`,
   `aldump.rs:61-70`, `:223`) are deleted. This closes #161: a workspace routine's digest
   effects include what its dependency callees do, and the tree's coverage is the context's
   (step 5's limit 5-A3 closes).
2. **One composition per view.** Where a plan has a cone today, the det cone and the witness
   pass are ONE `compose_cone_over_graph` call: derived rows and coverage for every routine of
   the view, raw facts only for the subjects, one `pd_solve` (G-1: neither retain nor
   re-solve).
3. **Scoped witnesses, never for a dependency subject (I5).** Subjects are primary routines
   (`Role::Primary`, #220). `RAW_INHERITED_FACTS`, `ConeOutput` and policy's command-local RAW
   build are deleted; `inherited_raw()` and the summary's raw field become `#[cfg(test)]`
   fixture input (6A-11's sibling, §4.8); policy reads the pass (Decision 16).
4. **The call-site ledger reads every caller** (`model.calls.edges`, then
   `rel.dependency_calls`, Decision 18), and a call resolved only into body-less dependency
   rows is `unfetched-dependency` (Decision 8, ledger half; the gap half is #220's).

## 2. Non-goals

- **fingerprint, prove, diff** stay single-app (step 9) and on a STANDALONE composition
  (`compose_snapshot(model)`, §4.4), whose counters do not move. Step 9 moves them onto the
  view's composition.
- **Roles, schema 4, the gap reachability filter**: #220, merged first.
- **No batch, no `JobId` map** (step 12): one job per view (6A-3).
- **No change to resolution or the L4 solve.** Histogram SHA equal; the L4 solve stays over
  `rel.primary()` (epic M5).
- **The guarantee "no unscoped cross-app raw" covers alsem's commands only.** `aldump
  --r3a5-cross-app-summary` (`aldump.rs:446` → `project_r3a5_cross_app`,
  `capability_cone.rs:4072-4140`) keeps composing raw for every routine of whatever workspace
  it is given, through `WitnessScope::All`; it is a diagnostic tool for fixtures, stated here as
  the one exception. `project_r3a3` likewise.
- **Discoveries deferred** (§12): #215, #216, #217, #227, #228 (kept as fallback F11).

## 3. Background (at `459294f1`)

### 3.1 The compositions today

| Command (cross-app) | det cone (`detector_context.rs:1387-1394`, over `rel.cone()`) | snapshot cone (`capability_cone.rs:4600-4609`, over the cut) | counters `cone`/`pd`/`scc` (`tests/cli/cli_build_counters.rs`) |
|---|---|---|---|
| analyze default | `DerivedOnly`, every routine | none | 1/1/2 (`:192`) |
| analyze `--detector d47` | none (no `SUMMARIES`/`TRANSACTION_SPANS`, `:1321`) | `RawOnly`, every routine, from `fill_ordering` (`detector_context.rs:1114-1121` → `digest.rs:4007-4010`) | 1/1/2 (`:233`) |
| digest | `DerivedOnly` | `RawOnly`, after `det` is released (`digest_cli.rs:1632-1639`) | 2/2/3 (`:335`) |
| prove (single-app) | `DerivedOnly` | `RawOnly` (`prove.rs:890`) | 2/2/3 (`:389`) |
| policy check | `DerivedOnly` (the run) + `Both`, EVERY routine incl. dependency rows (`gate/policy/pipeline.rs:266-272`) | none | 2/2/3 (`:624`) |
| fingerprint (single-app) | `DerivedOnly` | `RawOnly` (`fingerprint_cli.rs:498`; json path through `compose_full_snapshot`, `:437` → `snapshot_full.rs:163`) | 2/2/3 (`:444`) |

Both compositions call `compose_inherited_cones` (`capability_cone.rs:2580`). Its
`routine_ids` is both the emission set and the raw-walk set (`:2820-2823`); its `mode` gates
the raw walk (`:2837`, recursive SCCs) and the singleton's raw half
(`inherited_facts_for_singleton`, `:2886-2899`). The fact cones (`fact_cone_for_scc`,
`:2714-2716`) and coverage cones (`:2730-2743`) are built for every non-root SCC whatever the
mode, so a per-member raw gate changes no other routine's output. Every member's result goes
into one `out` map returned at the end (`:2939-2956`): all raw is alive together when the
composition returns.

### 3.2 What the tree's readers read

- digest S4 entries (`compute_digest_effects_cli`, `digest.rs:4085`): the root's own facts
  (`facts_by_routine.get(rid)`, `:3260`), every routine's DIRECT facts (`:243-315`), typed
  edges, the pd arena. Direct facts stay per routine of the view; only raw is scoped.
- the digest query (`run_digest_query_full_from_entries`, `digest_cli.rs:766`): `coverage`
  (`:779`), `identities` (`:789-800`) and `unresolved_cone(snap, rid)` (`:990`), a BFS over
  typed edges that collects every visited routine's `callsite_index` and
  `callsite_resolutions` rows (`unresolved_cone.rs:94-140`). Cross-app it walks into
  dependency routines, so it is the reader of dependency callers' ledger rows (D6).
- the ordering filter (`digest.rs:4026-4052`): `map_op` over every fact; a root is kept on its
  own facts, and the root is a subject.
- the ordering engine (`ordering_engine.rs:170-215`): `operation_index`, `callsite_index`,
  `callsite_resolutions` read only for the ROOT (`resolutions_from(routine_id)`, `:1028`),
  typed edges, and two maps built over every fact (`http_method_by_callsite_id`,
  `background_kickoff_callsite_ids`, `:202-214`). **Neither map can be read through a
  non-subject's fact** (Q3, decided statically, R1-20): the http lookup runs only when the IO
  chain's terminal routine is the root (`:1323-1333`), the background lookup only for the
  root's own call sites (`:1028-1050`), and an inherited fact's `witness_callsite_id` is its
  OWN subject's first hop (`retag`, `capability_cone.rs:1667-1673`). So a non-subject's
  inherited fact carries a non-subject's call site, never a root's. T3 keeps a break as
  evidence; F4 is dormant.
- root resolution (`resolve_changed_roots`, `digest_cli.rs:372`): `build_selector_indexes`
  (`selector_index.rs:84`) reads `snap.identities` and nothing else, and keeps primary ids
  only (`:112`); file and diff matching use `build_routine_span_index(model)` (P5).
- policy (`select_facts`, `policy_engine.rs:110-125`): `inherited_raw()` of primary routines
  only (`pipeline.rs:276-285`).
- the full snapshot (`compose_full_snapshot`, `snapshot_full.rs:137-189`) **[#220]**: the core
  plus `derive_permissions(resolved, &base)` (`:888`), which reads `base.cones[r].inherited` and
  `base.direct_full` for primary routines (`Role::Primary`, #220). Cross-app it also reads the
  cut base's `routine_to_stable` (`reachable_gaps`, `:507`; `filter_by_class`, `:184`).

### 3.3 The pd arena

`derive_pd_arena` (`snapshot.rs:1714-1800`) **[#220]** maps every cone state into the tree:
`EdgeIx` into the tree's sorted `typed_edges`, `producer_fact` into its `capability_facts`
(`expect`ed, `:1758-1760`). A cone link whose edge has no tree row is dropped and counted
(`PD_EDGE_UNMAPPED`, `:1777-1782`; a `debug_assert!(false)` in debug).

## 4. Design

### 4.1 D1: the scope type (one walk, raw gated per member)

`ConeOutput` (`cone_derived.rs:244-263`) is replaced by:

```rust
pub enum WitnessScope { None, All, Subjects(BTreeSet<String>) }   // internal routine ids
pub struct ConeScope { pub derived: bool, pub raw: WitnessScope }
impl WitnessScope { fn contains(&self, id: &str) -> bool }
```

In `compose_inherited_cones` every member of `routine_ids` gets its coverage record and (when
`derived`) its derived row, as today. The member's raw half runs only when
`scope.raw.contains(m)`: the member's mode is `(derived, raw.contains(m))`, passed where `mode`
is passed today (`:2837`, `:2877`, `:2886-2899`). Mode `(false, false)` (a non-subject of a
derived-free composition, e.g. `o`) skips the member's walk entirely, singleton and BFS; the
fact and coverage cones are built as before (R1-21). Old modes map 1:1: `DerivedOnly` =
`{true, None}`, `RawOnly` = `{false, All}`, `Both` = `{true, All}`.

**The output is split** (R1-01, R1-05): `ConeOutcome` gains `raw: HashMap<String,
Vec<CapabilityFact>>`, one entry per member whose raw half ran, and `cones` keeps coverage only
(`ConeResultPub.inherited` is removed). At the gate the walk counts
`build.witness_pass_members` and, by role, `witness.non_primary_members` (R1-15): these count
what was materialized, not what was asked for.

### 4.2 D2: the `caps` and `raw` cells, one composition per view (G-1)

`View` gains:

```rust
caps: Option<CapabilityAnalysis>,       // Cell::Caps
raw:  Option<HashMap<String, Vec<CapabilityFact>>>,   // Cell::Raw, subjects only
pub struct CapabilityAnalysis {
    pub direct_full: HashMap<String, Vec<CapabilityFact>>,   // det_cone_inputs, every routine
    pub coverage: HashMap<String, CoverageRecord>,           // every routine
    pub derived: ConeDerivedStore,                           // empty when !derived
    pub pd: Option<ConeArena>,                               // taken by the tree
}
```

`prepare` fills both right after `rel` with ONE `compose_cone_over_graph(rel.cone(), nodes,
&direct_full, &coverage_in, &ws.routines, ConeScope { derived: union & (SUMMARIES |
TRANSACTION_SPANS) != 0, raw: plan.witness })` (R1-22), the inputs from today's
`det_cone_inputs(model, rel)` (`detector_context.rs:1254-1293`, unchanged, so step 5's fold is
the one fold). A plan that needs neither derived rows nor witnesses (query) fills neither.

`build_detector_artifacts` (`:1300`) stops composing: it takes `caps` (direct, coverage,
derived) by move and never sees `raw`; it asserts so (I5). The summary assembly
(`:1408-1445`) is unchanged except its source. The resolved own entries (`capability_cone.rs:2650`)
stay a local of the one call: nothing is retained, nothing re-solves (G-1). The Cone-SCC pair
stays inside the composition (6A-2).

Rejected: (a) a separate witness composition (re-solve): a second full-graph pass on `d`
puts B1 out of reach. (b) Retaining `resolved` across cells: `ResolvedEntry<'g>` borrows the
graph (`pd_solve.rs:91-105`).

### 4.3 D3: the subject set and the tree demand, fixed before the composition

`Plan` gains `witness: WitnessScope` and `needs_tree: bool` (R1-06), set by the plan factory:

| Plan | `witness` | `needs_tree` |
|---|---|---|
| analyze without `ORDERING_FACTS`, events, query; fingerprint/diff/prove (standalone, §4.4) | none | no |
| analyze with `ORDERING_FACTS` (d47/d49/d51) | the internal ids of the rows of `reportable_roots(model)` (`digest.rs:3698-3718` returns stable ids, by #220's role rule; mapped back through `stable_routine_id`, every row of a collided id) | yes |
| digest | the changed roots, resolved before `prepare` (below) | yes |
| policy check | every `Role::Primary` routine (#220's rule; today `is_primary_routine`, `pipeline.rs:276-285`) | no |

Digest builds `derive_identity_table(model)` (`snapshot.rs:1385`, model only) before `prepare`,
resolves its roots from it (`resolve_changed_roots` takes `&SnapshotIdentityTable` instead of
`&CapabilitySnapshot`, P5) and passes them in the plan; the table moves into the tree. It is a
command-local, not a View cell, until step 12 (6A-11). A digest with no resolved root keeps
`WitnessScope::None`, `needs_tree: false`, and still exits 2 (`digest_cli.rs:1682`).

Ordering subjects are every reportable root (6A-6); F2 gates them by ingredient if Task 1 says
the raw is over budget.

### 4.4 D4: the tree, composed from `caps`, `raw` and `rel` (replaces `R3a3SourceBase`)

`compose_snapshot_from_base(model, &R3a3SourceBase)` (`snapshot.rs:1263-1303`) **[#220]**
becomes `compose_tree(model, typed: &[TypedEdge], dependency_calls: &[CallEdge], caps:
&mut CapabilityAnalysis, raw: HashMap<..>) -> CapabilitySnapshot`. `R3a3SourceBase` becomes a
struct of BORROWS with the same field roles, so each deriver keeps its body:

| Deriver (`snapshot.rs` **[#220]**) | Today reads | After |
|---|---|---|
| `derive_identity_table` (`:1385`) | model | digest's table (moved in) or a fresh one |
| `derive_capability_facts` (`:1456-1499`) | `base.direct_full` + `base.cones[*].inherited` (cloned) | `caps.direct_full` (borrowed) + `raw` **by move**: each subject's `Vec` is drained into snapshot facts and dropped as it goes, so raw exists once (Decision 24, R1-01) |
| `derive_typed_edges` (`:1514`) | `base.graph.typed_edges` (cut) | `typed` = `rel.cone()` |
| `derive_operation_evidence`, `derive_callsite_evidence` (`:1855`, `:1981`) | `base.ws_routines` (a clone, `capability_cone.rs:4614`) | `&model.workspace.routines`, borrowed |
| `derive_callsite_resolutions` (`:2070`) | `base.calls` (cut, a clone `:4542`) | `model.calls.edges` then `dependency_calls` (D6) |
| `derive_coverage` (`:2460`) | the snapshot's own fold over the cut (`:4562`) | `caps.coverage`: step 5's fold over `rel.uncertainty_all()` (closes 5-A3) |
| `derive_event_declarations`, `derive_root_classifications`, `derive_routine_order_frames` | base | model (`model.events.graph`, borrowed) |
| `derive_pd_arena` (`:1714`) | `base.pd_solution` | `caps.pd.take()`, scoped (D5); the cone arena is dropped as soon as the tree's arena is built (R1-27) |
| `derive_analysis_gaps` (`:2376`) | base | as #220 leaves it (its role rule, `rel.cone()` reach) |
| `derive_permissions` (`snapshot_full.rs:888`) **[#220]** | `base.cones[r].inherited` + `base.direct_full`, primary routines | the tree's inputs, same routines |

Deleted: `build_r3a3_source_only_base`, `_over`, the cut's clone of calls and event graph
(`capability_cone.rs:4541-4544`) and its own fold (`:4560-4593`), the timings mode, and the
"legacy cut" doc text (`:4510-4513`, `combined_graph.rs:309`).

**`compose_snapshot(model)` stays as the single-app convenience** for prove, fingerprint,
diff and tests (`digest.rs:3960`, `:3973`; `snapshot.rs:2731`). It asserts the model has no
dependency-role row (so the cut cannot come back under a new name, R1-09), builds the graph
with `build_combined_graph(&model.workspace, &model.calls, &model.events.graph)` as today
(counted `bcg`; no relation, so no extra Tarjan, R1-08), then the cone inputs over that graph,
one composition `{derived: false, raw: All}` and `compose_tree`. Single-app the graph's typed
edges are `rel.cone()`'s (P13), so its output is today's snapshot and its goldens do not move.
`det_cone_inputs` takes `&ExecRelation` (`detector_context.rs:1254`) only to read
`rel.uncertainty_all()` for the fold (`:1273`); it is split into a body that takes the
uncertainty edges (`impl Iterator<Item = &UncertaintyEdge>`) and the existing wrapper, and the
standalone path passes `&graph.primary.uncertainty_edges` (single-app the dependency half is
empty, so this is the same fold).

**`compose_full_snapshot`** (`snapshot_full.rs:137-189`) **[#220]**: single-app, through the
standalone path above. Cross-app (`SnapshotUniverse::CrossApp`, #220, reached only from tests
until step 9): over `base.rel.cone()` with `Subjects(every Role::Primary routine)`, which is
everything `derive_permissions` and #220's class filter keep (R1-09).

### 4.5 D5: the tree's pd arena, scoped (G-4, R6-4)

The tree's `PdArena` keeps today's shape (`EdgeIx` into the tree's sorted `typed_edges`,
`producer_fact` into `capability_facts`), so no digest reader changes (6A-5). It maps only the
states reachable from the subjects: start at `pd.by_routine[s]` for each subject, follow
`links[..].callee_state`, keep each state once, in state order. `enumerate_proofs` starts at a
root's states and follows links (`digest.rs:880-905`), so a state outside that closure is never
read; T3 pins it. With `WitnessScope::All` the closure is every state: today's arena.

**R6-4, the link rule.** `derive_typed_edges` skips a call-family or event-dispatch row that
has no anchor (`snapshot.rs:1571-1663`), and an unmodelled kind (`_ => continue`); an
`implicit-trigger` row is NEVER skipped, with or without an anchor (`:1672-1689`, #96 D4: the
cone follows it, so the arena needs its row). It records the keys it skipped; the exemption
means no implicit-trigger key is ever in that set. A cone link into a skipped row is
dropped and counted `PD_EDGE_UNMAPPED`, as in release today. The `debug_assert!(false)`
(`:1780`) becomes `debug_assert!(skipped.contains(&key))`: a link may miss only a row that was
skipped for want of an anchor (R1-28). T6 hand-states one; the CDO gate asserts
`PD_EDGE_UNMAPPED == 0` on `d` and `o` (F6 if not).

### 4.6 D6: the ledger reads every caller (Decision 18, M8; Decision 8 ledger half)

`ExecRelation::new` (`exec_relation.rs:69-93`) **[#220]** keeps `dependency_calls:
Vec<CallEdge>` (the `dependency` argument, today dropped after assembly), read only by
`derive_callsite_resolutions`, printed as a `rel.dependency_calls` census bucket. Its reader is
`unresolved_cone` (digest; §3.2), which walks into dependency routines: without these rows the
leftover-typed-edge pass (`snapshot.rs:2224-2268`) still writes a row for each dependency call
site, but always as `status: "resolved"`, `dispatch_kind: "direct"`, no
`result_consumed`/`under_asserterror`, so a dependency routine's unresolved or builtin call
would be misreported (R1-11). It lives in every cross-app view; its bytes are measured at Task
1 (P8) and F12 applies if they are large (R1-29).

In `classify_resolution` (`snapshot.rs:2280`) **[#220]**, a `"resolved"` call whose every
target is a body-less dependency row is `unfetched-dependency` with its edges (today only
`"opaque"` is, `:2352`). **This moves the ordering engine on its own**: such a ROOT call site
becomes a root boundary (`is_opaque`, `ordering_engine.rs:1044-1049`), so d47/d49/d51 can move
from D6 alone. Part 1 attributes each moved finding to D6 or to the cone graph (§8). Whether a
single-app model carries body-less rows (ABI stubs, `engine/deps/projection.rs:259`) is counted
at Task 1; if it does, a single-app `cmp` of §8 Part 1 may move only in findings D6 explains,
each listed. #220's T5 gains the ledger assertion (§11).

### 4.7 D7: cell order, effects and retention (G-2; epic §5.9 rule 3 as amended, 6A-1)

```
(1) program, graph job          (2) model, body released, rel
(3) caps + raw: ONE composition
(4) tree (needs_tree only): compose_tree takes raw by move, caps.pd by take   → Cell::Tree
(5) effects (Cell::Effects): digest = per root its S4 entry and its unresolved cone, plus the
    tree's identities and coverage; ordering = the ordering facts (the existing ordering cell);
    then release(Cell::Tree)
(6) det (moves direct, coverage, derived out of caps; no cone), then l4, harvest; release(Caps)
(7) runs; then the post-run projections (tx map, fanout, the policy rule engine off det and
    Cell::Raw, Decision 47); then release(Raw)
(8) compute: the digest query from Cell::Effects + tx map + l4   → model, rel, l4 released
```

| Cell | Filled | Read by | Released |
|---|---|---|---|
| `Caps` | (3) | (4), `det` at (6) | (6), after `det` |
| `Raw` | (3) | the tree at (4) (by move); policy's rule engine at (7) | (4) when `needs_tree`; else after (7) |
| `Tree` | (4) | (5) | (5) |
| `Effects` | (5) | digest's query at (8) | after (8) |

So no tree part meets `det` or `l4` (G-2 resolved): digest keeps only its per-root entries, the
roots' unresolved cones, `identities` and `coverage` (R1-07). `run_digest_query_full_from_entries`
takes those instead of `&CapabilitySnapshot`; `unresolved_cone` is computed at (5).
`policy_plan` gains `after_run: SUMMARIES` (its rule engine now reads `det.summaries`, R1-23),
and its raw (primary subjects) lives beside `det` until (7) as Decision 47 says.

**Policy** reads `Cell::Raw` instead of `inherited_raw()`; `select_facts` takes `(direct, raw)`
slices. Deleted: `pipeline.rs:255-272` (the RAW build), `substrate::RAW_INHERITED_FACTS`
(`registry.rs:78`), the assertion `session.rs:273-276`.

**The live-cell checkpoint** (R1-30, A6.18): at the first run, the census prints one
`view.live` line naming every filled cell; the test asserts it has no `Tree`, no pd arena and no
`Raw` (except policy's). Break: skip `release(Cell::Tree)` → the line names `Tree` → fails.

### 4.8 What is deleted or narrowed

| Item | Where |
|---|---|
| `ConeOutput` | `cone_derived.rs:244-263`; call sites `capability_cone.rs:4127`, `:4608`, `detector_context.rs:1375-1379`, tests → `ConeScope` |
| det's own cone | `detector_context.rs:1386-1406` (the call; the summary assembly stays) |
| `RAW_INHERITED_FACTS` | `registry.rs:32-90`, `session.rs:193`, `:273-276` |
| `inherited_raw` and the summary's raw `Option` | **narrowed to `#[cfg(test)]`** (`full_summary.rs:42-130`): production `FullRoutineSummary::new` has no raw parameter; `test_support::summary` (`test_support.rs:240-249`) and `cone_store_of` (`:30-55`) keep raw as fixture input; the raw oracles `may_commit`, `touches_db_of`, `touches_physical_table_of` (`capability_query.rs:62-102`, already `#[cfg(test)]`) keep checking the derived predicates (R1-10) |
| policy's RAW build | `gate/policy/pipeline.rs:255-272` |
| the cut graph | `capability_cone.rs:4485-4623`; `R3a3SourceBase` becomes the borrowed input struct |
| the full-graph timings mode | `capability_cone.rs:4625-4694`, `aldump.rs:61-70`, `:223`, and its smoke test `tests/cli/aldump_smoke.rs:100` |
| ordering's own snapshot | `digest.rs:4007-4010` (`compute_digest_effects_for_ordering_with` takes the tree) |

### 4.9 Counters (I1; the final row)

New counter `build.witness_pass` = 1 per composition that materialized at least one raw
`Vec`. Expected (parenthesised = base, P9; ws_dep fixtures):

| Row | `bcg` | `cone` | `v2` | `scc` | `pd` | `ctx` | `snap` | `witness` |
|---|--:|--:|--:|--:|--:|--:|--:|--:|
| analyze cross-app default | 1 | 1 | 1 | 2 | 1 | 1 | 0 | 0 |
| analyze cross-app / single-app d47 | **1** (2) | 1 | 0 | 2 | 1 | 1 | 1 | 1 |
| digest cross-app / single-app | **1** (2) | **1** (2) | 1 | **2** (3) | **1** (2) | 1 | 1 | 1 |
| policy check cross-app / single-app | 1 | **1** (2) | 1 | **2** (3) | **1** (2) | **1** (2) | 0 | 1 |
| prove, fingerprint json/query, diff | unchanged (standalone, §4.4) | | | | | | | 1 per standalone composition |
| query, events | unchanged | | | | | | | 0 |

"1 per view" holds on every row the step moves; prove/fingerprint/diff keep "stated 2" until
step 9 (6A-9).

### 4.10 Memory and the census

| Item | Lives | Bucket |
|---|---|---|
| raw of the subjects | (3) → (4) (policy: → (7)) | `raw` (`facts_tally`), plus `witness.non_primary_members=0` |
| `caps.pd` | (3) → (4) | `arena.link_graph` (exists) |
| the tree, scoped pd | (4) → (5) | `snapshot.*` (exist), printed at (4) |
| digest's effects | (5) → (8) | new `effects.digest` |
| `rel.dependency_calls` | the view | new `rel.dependency_calls` |

The census is shallow (lower bounds: owned strings are not walked, `snapshot.rs:1322-1346`), so
it names what is alive but cannot certify a fit (R1-04). The fit is read from measured process
peaks (§8.4 S2, B2, B4). Policy's peak should DROP (13.67 GiB today), reported in Part 3 and
checked in S2.

## 5. Test plan

Every new test lists its break (made with Edit, reverted with Edit), the failing assertion and
the revert; both outcomes are recorded (I9, the `discrimination-proof` skill).

- **T1 (#161)**: `dep_dispatch_fixture` (`tests/common/dep_dispatch_fixture.rs`) gains a
  variant whose `Inner` does `Cust.Modify()` (today `Insert()`, `:28`), so the fixture matches
  #161's text (R1-12). Cross-app `digest --routine CallChain`: DB_MODIFY on dependency Table 18
  and COMMIT, each with the path `CallChain → Outer → Inner`. Break: compose the tree over the
  primary callers' typed edges only (the cut) → both effects vanish → fails.
- **T2 (scope)**: R calls M calls writer W, subjects `{R}`: R's raw equals the `All` run's; M
  and W have none; every routine has coverage and a derived row. Break 1: gate no-op (`raw =
  All`) → "M has no raw" fails. Break 2: put the coverage record under the gate → W has no
  coverage → fails.
- **T3 (scoped-equals-unscoped oracle, Decision 14)**, test-only, per consumer: the production
  scope against an UNTOUCHED `WitnessScope::All` arm on the same view, comparing output bytes:
  digest on every root of every cli-b `c.diff` and on the dependency fixtures; the ordering cell
  (kept-root set and every `OrderingFacts`) on every r0 dir with the `Hyperlink` and rid-less
  write fixtures (Decision 38); policy's selected facts per primary routine (Decision 41,
  against the parent's `inherited_raw()` path, recorded as a fixture). CDO single-app as a
  CDO-gated arm. **Breaks apply to the scoped arm only** (R1-13): (1) under `Subjects`, skip the
  fact-cone build for non-subject SCCs (`capability_cone.rs:2714-2716`) → a subject loses
  inherited facts → fails; (2) under `Subjects`, `contains()` is false for recursive members →
  fails on a recursive r0 fixture. (3), evidence only: under `Subjects`, drop non-subject facts
  from the ordering engine's call-site maps; a green run records that the reader does not look
  past the subject on that corpus (§3.2).
- **T4 (single-app identity, no-dependency parity, E9)**: every single-app golden unchanged;
  for every r0 dir, cross-app digest/analyze-d47/policy equal single-app except the stated
  header fields. Break: give the standalone `compose_snapshot` `Subjects(∅)` → every inherited
  fact vanishes from the fingerprint golden → fails (R1-14).
- **T5 (one composition)**: `cli_build_counters.rs` rows restated to §4.9, the standalone rows
  included. Break: call the old det cone in addition → `cone 2, pd 2, scc 3` → fails.
- **T6 (R6-4)**: hand-stated edge in `rel.cone()` whose call site is absent from its caller's
  `call_sites`, carrying a pd link: no `typedEdges` row, the link dropped,
  `PD_EDGE_UNMAPPED == 1`, no panic in debug. Break: keep the row with a default anchor → the
  row-count assertion fails.
- **T7 (ledger)**: (a) a dependency caller's call site that is `builtin` or unresolved, with
  `result_consumed` set, appears in `callsiteResolutions` with THAT status and flag; (b) a
  resolved call into a symbol-only dependency routine is `unfetched-dependency`. Break (a):
  leave `dependency_calls` out → the leftover pass writes `resolved`, no flag → fails (R1-16).
  Break (b): keep the `"resolved"` arm → fails.
- **T8 (I5, coverage equality)**: `witness.non_primary_members == 0` on CDO `d` and policy (`o` struck by §14.7:
  its cut composition is a stated I5 exception)
  (counted at the gate, by #220's `routine_role`); on the dependency fixture
  `tree.coverage[r] == det.summaries[r].coverage` for every routine. Break 1: set policy's
  production scope to `All` → the materialized count is > 0 → fails (R1-15). Break 2: make the
  tree fold its own coverage over the primary callers' uncertainty (the cut's fold) → the
  equality fails on a routine whose only four-kind edge is in dependency code.
- **T9 (root resolution)**: only the `changedRoutines` selector path reads the identity table
  (`build_selector_indexes`, `digest_cli.rs:386`, `:398-423`); file and diff roots come from
  the span index (`:381`, `:436-510`). So T9 has two parts. (a) A USE pin on a ROUTINE-selector
  input: digest resolves `--routine <name>` from the table it passes into the plan. Break: pass
  a table with that primary routine's row removed → the selector finds no root → fails (R1-17).
  (b) Diff parity: on every cli-b diff fixture the roots equal today's (no break needed; the
  span index is unchanged code). A third assertion: the table is unfiltered (it holds the
  dependency fixture's dependency rows).

### 5.9 Tests deleted or rewritten with the code they pin (I2's exception)

Deleted: `session.rs:676-680` (`a det cell never carries RAW_INHERITED_FACTS`), the
`should_panic(expected = "RAW_INHERITED_FACTS")` test (`capability_query.rs:295`), the timings
mode's smoke test (`tests/cli/aldump_smoke.rs:100`). Rewritten: the `ConeOutput` parity tests
on `ConeScope` (`cone_derived.rs:1195-1780`); the uses of `build_r3a3_source_only_base` in
`detector_context.rs:2094`, `detector_context_step5_tests.rs:283` (its claim becomes T8's),
`detector_context_11b_tests.rs:1968`, `capability_cone.rs:6347`, `:6385`, on `compose_tree`;
`tests/cli/cli_events_policy_dependencies.rs:439` (names the RAW mutation) on `Cell::Raw`.
Unchanged: the 24 `test_support::summary`/`cone_store_of` call sites and the raw oracles (§4.8).
The plan lists each with its replacement.

## 6. Acceptance matrix

| # | Row 6 item | Evidence | Feasible on the measured data? |
|---|---|---|---|
| A6.1 | #161 acceptance | T1 | yes |
| A6.2 | context == snapshot coverage on the dependency fixture | T8 | yes |
| A6.3 | CDO digest wall and both peaks ≤ parent + band | B1, B2 | wall and process peak likely; post-attach peak **partial** (not measurable on Linux, 6A-8) |
| A6.4 | same for analyze with d47/d49/d51 | B3, B4 | **wall: not deliverable as worded** (§0 item 1); delivered as 6A-10's cost bound. Peak: unknown until Task 1 (S2) |
| A6.5 | counters at the final row | T5, §4.9 | yes |
| A6.6 | no raw `Vec` for any dependency subject | T8 (materialized count) | yes |
| A6.7 | CDO `policy check` moves nothing | Part 1 `cmp`, custom and bundled policy, cross-app | yes |
| A6.8 | stop rule | §8.4, after Task 1, before Task 2 | yes |
| A6.9 | `mk.py` test, revert to cut → fails | T1's break | yes |
| A6.10 | scope discrimination; scoped-equals-unscoped oracle with the whole-routine-gate discrimination | T2, T3 | yes on the corpus, the dependency fixtures and CDO single-app; **partial** on CDO cross-app (unscoped does not fit; Part 1's triaged diff is complementary evidence, not the oracle, Q6) |
| A6.11 | ordering root-set oracle (`Hyperlink`, rid-less) | T3 | yes |
| A6.12 | policy facts oracle | T3 | yes |
| A6.13 | `--diff` root resolution byte-equal | T9, T3's digest arm | yes |
| A6.14 | no-dependency parity (E9) | T4 | yes |
| A6.15 | `RAW_INHERITED_FACTS`, `ConeOutput::Both`, `build_r3a3_source_only_base` and the "legacy cut" text deleted; `inherited_raw` test-only | `rg` over `src/` outside `#[cfg(test)]` | yes |
| A6.16 | `unfetched-dependency` ledger rule | T7 | yes |
| A6.17 | R6-4 filter and link rule | §4.5, T6, CDO `PD_EDGE_UNMAPPED == 0` | yes, unless F6 |
| A6.18 | `pd` arena in view identity, never alive during a run (E18) | §4.7; the live-cell checkpoint test | yes |
| A6.19 | goldens: cross-app digest in `cli_digest_query_dependencies`; `r4_cross_app_program` and r4/r4f if D6 or the cone graph move d47/d49/d51 | moved, each move triaged (`golden-diff-triager`), attributed to D6 or the cone graph | yes |
| A6.20 | CDO deltas triaged | `cdo-measure` on `o` and `d`; > 30 % FP → root-cause fix (the three are opt-in) | yes |
| E14 | CDO ratchets | `bash scripts/cdo-gate` | yes |

## 7. False-positive and failure shapes

- **New d47/d49/d51 findings** from the cone graph (dependency writes and IO) are the point of
  #161; one whose dependency path is not executable from the root is a root-cause fix (F8).
  **Findings moved by D6** (a root call into a body-less dependency row becomes a boundary) are
  triaged apart: the boundary is correct when the callee has no body.
- **Changed digest coverage on CDO** (`complete` → `partial` where the only four-kind edge is
  in dependency code; step 5 counted 965 primary routines): correct since step 5; reported.
- **The scoped pd closure misses a state a proof needs**: T3's digest arm.
- **A dependency subject gets raw**: T8.
- **An anchorless edge carries a link**: T6; CDO must be 0 (F6).
- **A vacuous no-regression bound**: §8.1's void check.
- **Noise**: server1 is shared; the quiet-slot rules of §8.

## 8. Measurement plan

One CDO bench, run ONCE after the final panel, through `agentflow run --name bench --timeout
60 -- <cmd>`, in 10+11a's three-part shape (`/work/bench/160-6/bench-draft.sh`, updated for
6A-10). `WS` from `.agent/config.json` `cdo_ws`; `P` = the post-#220 master this branch is
rebased on, `H` = the final-panel head; release-fast from clean detached worktrees;
`RAYON_NUM_THREADS` unset; peaks by `maxrss.py` (kernel `VmHWM`, KiB).

**Quiet slot.** The load gate (1-min load ≤ 4, wait ≤ 15 min, then fail closed) runs before
Part 2 and before the Task 1 measurements; a load tick above 8 during Part 2 makes that run
void (the one re-run follows). The conductor schedules both when the other session runs no
bench or build.

- **Part 1 (identity and moves).** Histogram SHA equal. SARIF `cmp`: cross-app default analyze
  and `--single-app` analyze, both scopes. Single-app `analyze --detector d47,d49,d51`: `cmp`,
  except findings D6 explains (§4.6), each listed. `policy check` cross-app, custom and bundled:
  `cmp` (A6.7). Single-app `fingerprint --format json`: `cmp`. DIFFED and triaged (A6.20), each
  move attributed to D6 or the cone graph: cross-app `o` SARIF and cross-app `d` JSON.
  `ALSEM_RETAINED_CENSUS=1` per side: §4.10's buckets and the I5 line.
- **Part 2 (bounded).** `bench_ab.py run --rounds N` (N per §8.1), interleaved, `d` and `o`;
  then 3 rounds per side and command through `maxrss.py`.
- **Part 3 (Decision 58: 3 rounds).** Cross-app `policy check`: wall reported, kernel peak
  bounded by B5 (expected to DROP); cross-app default `analyze`: reported, no bound.

### 8.1 Bounds (bands from this bench's parent runs; I6, Decision 20)

The band of a timer is max − min of the parent's runs of that timer. **Rounds** (R1-26): in
the Task 1 slot the conductor runs 5 parent rounds of `o` and `d` and records each band. If
both are ≤ 5 % of their medians, Part 2 runs 5 rounds and the band is max − min. If either is
over, Part 2 runs 7 rounds and every band is the trimmed max − min (each side's single highest
run dropped), pre-stated in the ledger before Task 2.

| Bound | Reading | Rule | Void check (read first) |
|---|---|---|---|
| B1 (A6.3 wall) | `d`'s process wall | `Hd ≤ Pd + Bd` | `Bd ≤ 0.05 × Pd`; no Part 2 load tick > 8 |
| B2 (A6.3 peak) | `d`'s kernel peak | `Hm ≤ 1.03 × Pm` | none |
| B3 (A6.4 wall, 6A-10) | `o`'s `analyze.total` span, its band on the same span | `Ho ≤ Po − S_P + K + T + E′ + Bo` (§8.4) | `Bo ≤ 0.05 × Po`; load as B1 |
| B4 (A6.4 peak) | `o`'s kernel peak | `Hm ≤ 1.03 × Pm` | none |
| B5 (policy peak, R1-24) | cross-app `policy check` kernel peak, Part 3's 3 rounds | `Hm ≤ 1.03 × Pm` (as B2/B4, R3-02) | none |
| — post-attach peaks | not readable on Linux (#204) | reported "not measured" (6A-8) | — |

A void check → re-run Part 2 once (fresh rounds), read every bound from the re-run alone;
void again → the bound FAILS (F9). Missed (met-not-void) → F10.

### 8.2 Numbers file

`docs/measurements/2026-10-<dd>-issue-160-step6.md`: 10+11a's sections plus the Task 1 probe
numbers and the stop rule's reading.

### 8.3 Expected (not a claim)

`d`: − the cut base (≈ 0.95 s at the base) − the cut derive (0.99 s) + T(3 roots) + the raw
walk of 3 roots. `o`: + K + T + E′ − S_P, several seconds.

### 8.4 The stop rule (Decision 52), read after Task 1, before Task 2

Task 1 lands the scope type, the split output and the cells behind the old wiring (nothing
switched), plus a measurement-only `aldump --issue160-step6-probe <ws>` (deleted in Task 2's
commit), on the post-#220 parent. The probe runs the HEAD pipeline up to the effects, in the
head's exact configuration (R1-02): the cross-app base, `det_cone_inputs`, one composition over
`rel.cone()`, the tree over the cone graph (with `dependency_calls` and the scoped pd), and the
effects. It runs once per configuration: `d` = `{derived: true, raw: the 3 roots}` with the
digest effects; `o` = `{derived: false, raw: reportable roots}` with the ordering cell; policy
= `{derived: true, raw: every primary routine}`, no tree. Each prints: subjects, materialized
raw `Vec`s and facts, `non_primary_members`, **K** = the input assembly
(`det_cone_inputs`, `detector_context.rs:1254-1293`) plus the composition, both timed (R1-02),
the tree derive ms (**T**), the effects ms (**E′**: for `d` the S4 entries AND the roots'
unresolved cones; for `o` the ordering cell), scoped pd states/links, `PD_EDGE_UNMAPPED`,
anchorless rows with links, ordering kept roots, `dependency_calls` count and bytes, single-app
body-less rows. Each runs 3 rounds through `maxrss.py` in the quiet slot.

**S_P** is measured by the same probe's PARENT-SHAPE arm (at Task 1 the cut builder still
exists behind the old wiring), on the same binary and slot, 5 rounds: for `o`, the cut base +
the cut derive + the ordering cell over the cut tree; for `d`, the cut base + the cut derive +
the parent's effects, i.e. `compute_digest_effects_cli` over the roots (`digest_cli.rs:1655`)
and their `unresolved_cone`s (`:990`) over the cut tree (R2-01). For `d` the parent's det cone
(DerivedOnly over `rel.cone()`, with its inputs) is the arm's **D_P**. The ledger records K, T,
E′, S_P and D_P (medians and bands) before Task 2.

- **S1 (wall).** `d`: `K_d − D_P + T_d + E′_d − S_P(d) ≤ Bd`. `o`: `K_o + T_o + E′_o −
  S_P(o) ≤ 0.5 × Po` (the cap of 6A-10).
- **S2 (memory).** The probe's process peak per configuration (KiB, `maxrss.py`), against the
  parent's process peak of the same command: `o` and `d` ≤ 1.03 × parent (R1-04). The probe
  stops at the effects, so its peak is a lower bound for the command's; B2/B4 bound the whole
  command. **Policy** (R1-24): its `Raw` overlaps `det`, `l4` and the rule evaluation (§4.7),
  which the probe does not reach, so its peak is bounded on the final command instead: B5,
  cross-app `policy check` kernel peak ≤ 1.03 × the parent's (13.67 GiB at the base), read in
  Part 3.
  The probe still prints policy's composition peak as an early warning: over the parent's
  whole-command peak → F13 before Task 2.
- S1 or S2 fails → **F2**, then F11, re-projecting after each. Still failing → the claim stops
  at Task 1 (scope type, split output and cells merged behind the old wiring, nothing
  switched), files "step 6: cone-graph witnesses exceed budget on CDO" with the numbers, and the
  epic re-plans. No bound is relabelled.

## 9. Amendments this spec asks the epic to take

- **6A-1 the witness pass is the raw half of the ONE composition**, not a second `RawOnly`
  call; §5.9 rule 3 items (3)-(6) become §4.7's (3)-(6). G-1: neither retain nor re-solve.
  The det builder loses its cone; Decision 15 holds.
- **6A-2 the Cone-SCC pair stays in the composition** (M3 withdrawn).
- **6A-3 one job per view until step 12**; the `BTreeMap<JobId, SnapshotTree>` arrives then.
- **6A-4 Decision 8's gap half is #220's**; step 6 does the ledger half.
- **6A-5 the arena's `EdgeIx` indexes the tree's sorted `typed_edges`**, scoped to the
  subjects' state closure.
- **6A-6 ordering subjects are the reportable roots, not membership-gated**, unless F2 fires.
  *Void since 6A-12* (§14.7): the ordering plan has no witness scope.
- **6A-7 step 6's wall bounds are no-regression bounds with a band-width void check**
  (`B ≤ 5 % of P`), with §8.1's pre-stated round rule.
- **6A-8 10A-4 carried**: post-attach peaks are reported "not measured" until #204.
- **6A-9 prove, fingerprint and diff stay on a standalone composition until step 9**, with
  today's counters ("stated 2"); their scoped-equals-unscoped arms move to step 9.
- **6A-10 analyze with d47/d49/d51 carries a pre-stated COST bound** (a change to E15's "6
  within parent", for `o` only): `Ho ≤ Po − S_P + K + T + E′ + Bo`, every term measured at
  Task 1 in the head's exact shape on the post-#220 parent, and the stop rule caps
  `K + T + E′ − S_P` at `0.5 × Po`. Reason: the parent composes over the cut and the head must
  compose over the cone graph; keeping the ordering cell on the cut would keep the cut graph
  the row deletes. B4 stays as worded. *Void since 6A-12* (§14.5): `o` is a no-regression
  bound again.
- **6A-11 `identities` is a command-local of digest until step 12**, built before `prepare`
  and moved into the tree; with one job per view a cell has no second reader.
- **6A-12 the split** (§14.2): step 6 moves digest onto the one composition over
  `rel.cone()`, with the DAG witness (§14.3) and the stated cost bounds B1′/B2′ (§14.4, a
  change to E15 for `d`); the ordering cell (d47/d49/d51) stays on the cut graph, which stays,
  until a #161 follow-up; policy's Task 3 stays only if B5 holds (§14.5).

## 10. Fallbacks (every "if the measurement shows X")

Every fallback that changes code re-runs every identity gate it could affect (T3, T4, the
golden targets, §8 Part 1) and the cost and scope checks (S1, S2, T8) before the claim
continues; each is recorded in the ledger.

| # | If | Then |
|---|---|---|
| F1 | — | withdrawn in round 1: raw is all alive when the composition returns, so releasing it after use cannot lower the peak (R1-01) |
| F2 | S1 or S2 fails on `o` (first rung) | gate the raw walk by three ordering bits (Decision 38, `map_op`'s vocabulary). **The derived row does not have them today** (R3-01): its flags are table, commit, http, file and physical table (`cone_derived.rs:116-129`) plus the write-op mask (`:132-136`); there is no UI-sink bit, and its http/file bits key on `resource_kind`, not on `map_op`. So F2 first ADDS three `map_op`-keyed bits to the row and its fold (external IO = `HTTP`/`FILE`, UI sink = `UI_CONFIRM`/`UI_MESSAGE`/`UI_WINDOW_OPEN`, DB write = `DB_INSERT`/`DB_MODIFY`/`DB_DELETE`: the filter of `digest.rs:4026-4052`). Today a member's derived fold and its raw picks are built in ONE pass (`capability_cone.rs:2824-2931`: the singleton call `inherited_facts_for_singleton` does both under one `mode`; the BFS member's fold follows its walk), so the bits are known only after the walk. F2 therefore runs each subject FOLD FIRST: the member is composed with mode `(true, false)`, its row's three bits are read, and only a member with an ingredient is composed again with mode `(false, true)` for its raw (R2-02). The second walk runs inside the SAME SCC member loop of `compose_inherited_cones`, while the successor fact cones it reads are still alive (a cone is freed only after its last predecessor's member loop, `capability_cone.rs:2670-2701`), so it stays one composition and one `pd_solve`. The extra cost is the new bits, the derived fold of every subject and a second singleton/BFS pass for the kept ones; Task 1's probe prices all three as a fourth configuration (`o` under F2: K, T, E′ and peak). T3's ordering arm proves the kept-root set unchanged |
| F3 | — | promoted into the design (§4.7, R1-07) |
| F4 | T3's break (3) or a later reader is found to look past the subject | widen THAT reader's input to the subjects' callee closure (from `rel.cone()`), checked by T3's untouched oracle; never give a dependency routine raw (I5). Dormant: §3.2 shows no such reader today |
| F5 | D1 cannot be made byte-identical on derived rows (Part 1 default-analyze SARIF differs) | a defect; not closed in one attempt → **stop at Task 1** and re-panel: a second composition contradicts A6.5 and 6A-1 (R1-18) |
| F6 | `PD_EDGE_UNMAPPED > 0` on CDO, or a dropped link carries a proof | triage the edge kind; fix the anchor source; not closed → stop at Task 1 |
| F7 | CDO `policy check` differs | a defect; not closed in one attempt → stop, nothing merges |
| F8 | new d47/d49/d51 findings > 30 % FP on the triage sample | root-cause fix first (the detectors are already opt-in) |
| F9 | B1 or B3 void twice | the bound FAILS: claim blocked |
| F10 | B1-B4 met-not-void but missed | name the miss (`trace_summary.py` span, census bucket); ONE fix from F2, F11 or F12; re-run Part 2 once; still missed → FAILS |
| F11 | S1 short by the cross-app `direct=` time | #228: the cross-app `caps` fill reuses `build_cross_app_base`'s `direct_full`/`direct_coverage` (step 5's T5 pins the maps equal) |
| F12 | `rel.dependency_calls` > 50 MB on CDO (P8) | keep it outside the relation, in the cross-app base, read only by `compose_tree`, dropped after (4) |
| F13 | B5 misses, or the probe's policy composition peak already exceeds the parent's whole-command peak | run policy's rule engine right after (3), before `det` (the alternative of Q4), so `Raw` never overlaps `det` and `l4`. `det.summaries` does not exist yet then (R3-02), so the engine's `PolicyModel` is built from the cells that do: the summaries from `caps.direct_full` + `caps.coverage` (the values `det` would move into `FullRoutineSummary`), the raw from `Cell::Raw`, the events from `model.events.graph`; `Raw` is released right after, and `det` then takes `caps` as usual. Re-measure B5 once; still over → FAILS |

## 11. What #220 hands this step (re-cite after it merges)

- Every **[#220]** citation and file of the header list.
- (a) `compose_full_snapshot` keeps `opts.universe` and the class filter; the scoped tree must
  produce exactly the rows that filter keeps (#220's T9/T10 are the oracle; §4.4). (b) #220's
  T5 gains the `unfetched-dependency` ledger assertion. (c) #220's T9d gains "`Inner`'s
  operation is referenced by `CallChain`'s inherited fact, so it is kept". (d)
  `derive_analysis_gaps` keeps #220's role rule. (e) Subjects use `Role::Primary`; #220's
  `reportable_roots` already does.

## 12. Discoveries: in scope or deferred

| Issue | Verdict | Reason |
|---|---|---|
| #215 per-row cone and solve | deferred | changes per-id semantics; needs its own triage; row 6 does not need it |
| #216 guard frames of `#if` arms | deferred | rides on #215 |
| #217 twin-node fan-out | deferred | D6 adds dependency callers' rows but does not change how an edge picks a row; routing to every row moves results and needs its own triage |
| #227 dependency L4 solve | deferred | row 6 changes no L4 solve; it moves d1/d2/confidence readers; best after step 9 |
| #228 one cross-app producer | deferred, kept as F11 | not needed for correctness |

## 13. Open questions (recommended answers)

1. **Q1. 6A-10: accept a stated cost for `o`, as a change to E15?** *Recommended: yes*, with
   the terms of §8.4 and the cap `0.5 × Po`. This needs the operator's (epic's) decision.
2. **Q2. 6A-1: one fused composition?** *Recommended: yes*; F5 stops rather than licensing a
   second pass.
3. **Q3. Does a reader look past the subject?** *Decided: no*, statically (§3.2). T3's third
   break is kept as evidence; F4 is dormant.
4. **Q4. Policy timing (Decision 47).** *Recommended: keep it*; S2 checks policy's peak.
5. **Q5. 6A-9.** *Recommended: yes*, with the standalone path on `build_combined_graph` so the
   counters stay.
6. **Q6. The CDO cross-app oracle.** *Recommended:* single-app CDO plus the corpus and the
   dependency fixtures carry the oracle; the triaged cross-app diff is complementary evidence.
7. **Q7. Void-check noise.** *Recommended:* §8.1's pre-stated round rule; a twice-void bound
   fails (F9).
8. **Q8. `build.snapshot` per tree composition**, policy 0. *Recommended: yes.*
9. **Q9. Ordering subjects ungated at the start.** *Recommended: yes*, F2 is the first rung if
   Task 1 says otherwise.

Q1 and Q9 are void since amendment 6A-12 (§14.7): `o` is a no-regression bound again and the
ordering plan has no witness scope.

## 14. Amendment 6A-12: the split, and the DAG witness

Added 2026-10-23 at attempt 8, after Task 1's stop rule failed (ledger `## Task 1 STOP`).
**Revised the same day after the §14 panel round 1** (register entries A1-01 to A1-28) and
the operator's decision **B** for digest's output (§14.3.5): witnesses stop at the first
dependency routine, and effects reached only through a dependency call are grouped per
(boundary call site, effect type). **Where §14 and §0-§13 disagree, §14 wins**; §14.7 lists
every item it voids, keeps or changes. Citations in §14 are at master `6a2b7c8b` (the attempt-8 base; it differs from
`809fe684` only by the version bump in `CHANGELOG.md`, `Cargo.toml`, `Cargo.lock`) unless a
commit is named. No number here is measured on `6a2b7c8b` yet: every number is from the Task 1
bench (parent `7afa91a3`, `/work/bench/160-6/task1/m/`) or the witness prototype (scratch build
`/work/bench/160-6/diag-wt`, uncommitted on `2a4a867f`; scripts and output
`/work/bench/160-6/w6/`). Task 1′ re-measures them (`/work/bench/160-6/split/probes-todo.md`).

Words used below (beyond the spec's own):
- **First hop** = the root's own call site, operation or event dispatch that a fact's path starts with
  (`fact_first_hop`, `FirstHop::{Callsite, Op, Event}` in `digest.rs`).
- **Carrier** = a routine that holds the fact as a DIRECT fact (`is_bfs_carrier`).
- **DAG** = directed acyclic graph. Here: the graph of all shortest paths from a first hop.
- **E′** = the effects step (S4 entries plus the roots' unresolved cones), as in §8.4.

### 14.1 Why the spec changes

Task 1's stop rule failed (ledger `## Task 1 STOP`): on the cone graph, `d`'s E′ was 147,893.7 ms against a 200 ms
allowance, and `o` ran over 35 minutes and 22.9 GB. The cost was the per-fact witness search in
`reconstruct_witness_paths` (`digest.rs:1121`): one simple-path BFS per inherited effect fact, each
able to expand up to `MAX_STATES` = 25,000 states (`digest.rs:418`). On the cone graph a root inherits every write and
commit its dependency callees make, so `d`'s 3 roots carry 15,983 effect facts (the cut: 138), and 4,627 of those
searches hit the state limit or truncated (diagnostic build, ledger `## Timeline`).

A prototype replaced the per-fact search with one BFS per (root, first hop) that keeps every shortest-path predecessor
(§14.3). It cut `d`'s E′ to about 6 s, but `o` stayed at 285 s E′ and 25.2 GB peak: its composition alone
(K 14.9 s + T 25.9 s for 5,479 subjects and 8.48 M raw facts) is past the 0.5 × Po cap of 6A-10. No witness design
can bring `o` back, so the operator chose to split the step.

The §14 panel round 1 then found three things the first text missed (scratch prototypes `w6/proto4.sh` to
`proto9.sh`, outputs `w6/p4/` to `w6/p9/`):
1. **The peaks were taken under the wrong allocator.** The probe runs in `aldump` (system malloc); production `alsem`
   runs mimalloc with purge delay 0 (`alsem.rs:28`). Every peak in §14 is now taken under mimalloc (§14.6).
2. **A digest of an ordinary file did not fit.** `CDOUnhandledSalesPagesMgt.Codeunit.al` (88 roots) with the
   shortest-path witnesses alone: 301,356 effect facts, E′ 77.0 s, peak 22.5 GB. Digest's cost scaled with its
   OUTPUT: every dependency write a root reaches became its own effect with full witness paths.
3. **Digest's per-root ordering pass was quadratic.** `compute_ordering` materialized every happens-before pair of an
   owner group and cloned the group's ops into every occurrence (§14.3.7). Cutting witnesses at the boundary alone
   (option A) made it worse: `d` peak 28.3-29.0 GB, the 88-root file killed at 32 GB.

Option B (§14.3.5) plus the on-demand happens-before test (§14.3.7) brings `d` to E′ 3.1 s at the parent's peak and
the 88-root file to E′ 21.2-22.0 s at 3.1 GB (§14.4).

### 14.2 6A-12: the split

**6A-12 (text for §9).** Step 6 moves **digest** onto the view's one composition over `rel.cone()` with the DAG
witness (§14.3), under stated cost bounds (§14.4). **The ordering cell (d47, d49, d51) stays on today's cut graph**:
`fill_ordering` keeps `compute_digest_effects_for_ordering_with`'s own `compose_snapshot` (`digest.rs:4002-4010`),
the cut builder (`build_r3a3_source_only_base`, `capability_cone.rs:4506`; `:4513` on the branch after
`edca9f0c`) stays, and `o`'s bounds go back to no-regression bounds. Moving the ordering detectors onto the cone
graph is a follow-up under #161, which stays open. Policy's switch to `Cell::Raw` (Task 3) stays in step 6 only if it
meets its own peak bound B5 (§14.5).

What it narrows in the epic's row 6 (`2026-10-18-issue-160-analysis-session-design.md:637`):

| Row 6 item | After 6A-12 |
|---|---|
| snapshot on `Transfer::Cone` | digest's tree only, read with boundary witnesses and grouped boundary effects (§14.3.5). The cut builder keeps these readers (A1-17): the ordering cell (cross-app and single-app); prove, fingerprint and diff through `compose_snapshot` (single-app, where cut = cone, P13; 6A-9 unchanged); `compose_full_snapshot` cross-app (tests only until step 9); `compute_digest_effects` / `_with_ordering` (`digest.rs:3960`, `:3973`, the r4f projections); and a digest run whose plan includes an ordering detector (the ordering cell inside it) |
| `WitnessScope`; one composition per view; G-1 | unchanged for digest, analyze-with-summaries and policy. The ordering plan has no witness scope (6A-6 void). **Exception (A1-18):** an analyze run with an ordering detector AND a detector that needs summaries (e.g. the transaction-integrity preset) composes twice: the det composition over the cone graph and the ordering cell's cut composition; §14.10 states its counter row |
| `pd` arena in view identity; effects cells; `identities` cell (6A-11); G-2 retention | unchanged (digest is the one tree reader) |
| `RAW_INHERITED_FACTS`, `ConeOutput::Both`, `inherited_raw` deleted | `ConeOutput` deleted (Task 1 already did); the other two only with Task 3 (§14.5) |
| `LegacyCut` and `build_r3a3_source_only_base` deleted | **not in step 6.** The builder stays with one cross-app production reader, the ordering cell; its doc text says so and names #161. `aldump --issue160-full-graph-timings` is still deleted |
| `unfetched-dependency` ledger rule (D6) | on the cone-graph tree only, as Task 1 built it (`TreeInputs::bodyless_dependency`, `None` on the cut). The cut keeps today's ledger, so D6 cannot move d47/d49/d51 |
| R6-4 filter and link rule | unchanged (digest's tree; `PD_EDGE_UNMAPPED` 0 measured on `d`) |
| #161 acceptance | the digest half (T1), in the grouped shape: `CallChain` reports `DB_MODIFY` and `COMMIT` through the boundary call into `Dep Lib.Outer`, with Customer (18) in the group's resources (§14.3.5). The d47/d49/d51 half moves to the #161 follow-up |
| CDO digest wall and both peaks ≤ parent + band | **replaced by the cost bounds B1′ and B2′** (§14.4), a change to E15's "6 within parent" for `d` |
| same for analyze with d47/d49/d51 | **restored as worded** (B3, B4 no-regression); 6A-10 void |
| counters at the final row | §14.10 |
| no raw `Vec` for any dependency subject | holds for every composition step 6 adds (digest, policy). **I5 exception (A1-06):** the cut composition the ordering cell keeps is `{false, All}` over every model routine, dependency rows included (`capability_cone.rs:4594-4609`), as today; it stays until the #161 follow-up moves the ordering cell. T8 and A6.6 therefore read `d` and policy only, not `o` (§14.7). If Task 3 is reverted (§14.5), policy's RAW build is a second exception, stated in the merge's CHANGELOG entry |
| CDO `policy check` moves nothing | unchanged |
| oracles: scoped-equals-unscoped (digest, prove, fingerprint, ordering) | digest arm kept; the ordering arm and the ordering root-set oracle (`Hyperlink`, rid-less) move to the #161 follow-up; prove/fingerprint arms stay with step 9 (6A-9) |
| goldens: `r4_cross_app_program`, r4/r4f if d47/d49/d51 move | must NOT move (§14.3.4, A6.22) |

### 14.3 D8: the DAG witness (replaces the per-fact search)

#### 14.3.1 The rule

For a root `r` and a first hop `h` of `r`:

1. **Seeds.** The targets of `r`'s edges for `h` that carry a hop (`edge_has_hop`), sorted by routine id as today
   (`seed_pairs.sort_by`, stable). All seeds are at distance 1.
2. **One BFS per (r, h)**, over `idx.outgoing_edges` (pre-sorted by `edge_compare`), following only edges with a hop,
   never entering `r`, expanding no node at distance `MAX_DEPTH` (64). For each reached routine `v` it records its
   distance `δ(v)` and EVERY predecessor `(u, e)` with `δ(u) = δ(v) − 1` (a seed's predecessor is `(None, e)`).
   This is breadth-first search with a virtual source joined to every seed (CLRS, *Introduction to Algorithms*, 3rd
   ed., §22.2; 4th ed. §20.2). BFS computes the true shortest distances (Theorem 22.5), and an edge `(u, v)` lies on a
   shortest path to `v` exactly when `δ(v) = δ(u) + 1`. CLRS keeps one predecessor per vertex (the breadth-first tree,
   Lemma 22.6); keeping all of them gives the predecessor DAG, whose source-to-`v` paths are exactly the shortest paths
   to `v`. Shortest paths are simple, so the old per-path visited set is not needed.
3. **The DAG depends only on (r, h), never on the fact.** It is built on first use and shared by every effect fact of
   `r` with first hop `h`, in a per-root `DagCache` (a map `(first-hop kind, id) → DAG`), separate from the
   per-root hop memo `HopCache` (`digest.rs:1013-1043`, which stays an edge → hop memo; A1-27). Digest owns one
   `DagCache` per root (`digest_one_root`). **The fingerprint query owns one per root too** (A1-05): today
   `reconstruct_witness_paths_pub` builds a fresh `HopCache` per call (`digest.rs:4608-4615`); it takes the caller's
   `&mut HopCache` and `&mut DagCache`, and `fingerprint_query.rs:560-600` holds one pair per root across that root's
   rendered facts. Test W8 pins the build count.
4. **Per fact `f`:** `C_f` = the carriers of `f` (`is_bfs_carrier`, the `(op, kind)` bucket, as today). `dmin` = the
   least `δ(c)` over reached `c ∈ C_f`. The witnesses are the DAG paths from a seed to a carrier at `dmin`. A
   carrier at `dmin` has no carrier before it on its shortest path (that carrier would be nearer), so "stop at the
   first carrier on the path" is kept. **Which paths, in which order** (A1-01): the paths are enumerated in the
   order the merge sorts them, so the capped list is the true prefix. Backward from the carriers at `dmin`, the DAG
   nodes that lie on a shortest path to one of them are marked; a forward depth-first walk from the seeds visits
   only marked nodes and takes successor edges in the order of their hop JSON (the merge's tiebreak,
   `ProjPath::key_cmp`). All these paths have length `dmin + 1`, so this order is the merge's (length, JSON)
   order. The walk stops after `cap + 1` paths, where `cap` is the CALLER's cap (A1-04: `HARD_PATH_CAP` = 256 for
   digest and prove; the user's `--witness N` for the fingerprint query, `fingerprint_query.rs:575`, `:596`).
   `truncated` holds when `cap + 1` were found; `path-limit-reached cap=N` is pushed then, as today.
   Opaque-boundary seed paths (A1-21) are found as today (call-site and op seeds only, 1 hop, before any carrier
   path because they are shorter) and count toward `cap + 1`. A non-seed opaque routine is reached by the DAG (no
   `valid_nodes` prune) but is never reported: only carrier paths are read off the DAG. Test W9 pins both.
5. **What does not change.** Direct facts (case A), the root's own proofs (`enumerate_proofs`, `digest.rs:880`, which
   keeps its own `MAX_STATES` limit and its `state-limit-exceeded` diagnostic), the opaque-boundary report for
   call-site and op seeds, the event-hop rule (a subscriber that reaches no carrier gives no path: the old code pruned
   it by `valid_nodes`; the DAG gives it no path because only carrier paths are read), the first-hop contract, the
   merge, the sort, `MAX_PATHS` = 3 printed paths per effect, and `HARD_PATH_CAP` as the pre-capping bound.
6. **The depth limit is per fact** (A1-07). The DAG build records the nodes it did not expand at `MAX_DEPTH` (the
   depth frontier). For fact `f`, the outcome carries today's `depth-exceeded` diagnostic and `incomplete` exactly
   when `f` has no carrier path in the DAG, no proof path (`enumerate_proofs`), and a carrier of `f` is reachable
   from a frontier node (checked against `f`'s carrier set by a reverse walk from the carriers, bounded by the
   frontier; the walk is built only for facts with no path). A fact with a found witness never carries it. This is a
   stated change: today `depth_exceeded` is set whenever the search pops any node past 64 before its cut, even when a
   witness was found (`digest.rs:1473-1481`, `:1660-1667`). Tests W6 (no path, carrier past 64 → diagnostic) and W6b
   (carrier at 2 hops plus an unrelated 70-hop branch → no diagnostic; a fact whose only carrier is on a different
   deep branch → no diagnostic for this fact).

**One search rule for every consumer.** `reconstruct_witness_paths` serves digest, prove
(`compute_digest_effects_cli`), fingerprint's query (`reconstruct_witness_paths_pub`, `digest.rs:4594`, called at
`fingerprint_query.rs:575`, `:596`) and the ordering cell (`digest_query_with` with `ordering_witness_only`). The DAG
replaces the per-fact search for all of them; keeping the old search beside it would be two answers to one question.
The ordering cell runs the DAG (A1-28) at a measured neutral cost (§14.3.4). **The boundary rule and the grouping
(§14.3.5) apply only to digest's cone-graph tree**: they are switched on by a flag the tree carries, so the cut
(ordering, prove, fingerprint, diff, r4f) never sees them.

**Cost.** Per root: one `O(V_h + E_h)` BFS per distinct first hop `h`, plus path enumeration bounded by
257 × 64 hops per fact. The old cost was up to 25,000 expanded states per fact, times every effect fact. The new
cost grows with the number of first hops, not with the number of inherited facts.

**Rejected:** (a) capping the per-fact enumeration at `MAX_PATHS` instead of the caller's cap: it would change
`viaPathsTruncated` and conditionality inputs for a reason unrelated to the witness rule (a cap is the caller's
contract, A1-04). (b) Counting shortest paths by dynamic programming instead of enumerating `cap + 1`: it answers
`truncated` but not the printed prefix; the ordered walk of item 4 answers both. (c) One DAG per root for all first
hops: breaks the first-hop contract.

#### 14.3.1a The streaming top-k merge (A1-02)

The merge (`merge_normalize_via_paths`, `digest.rs:3164-3226`) keeps every deduped pre-capping path of an effect
(`AccumulatedEffect::all_paths`, `all_path_conds`, `:3283-3291`) until the root is done, although every output it
produces is a fold:
- the printed paths are the first `MAX_PATHS` distinct paths in (length, JSON) order;
- `viaPathsTruncated` is "an input was truncated, or more than `MAX_PATHS` distinct paths exist" (`:3215-3222`);
- conditionality is `effect_conditionality` over all distinct paths (`conditionality.rs:90-122`): the least rank
  (ranks are distinct, `:32-40`, so the least-rank value is unique), plus "has an unconditional path"; dedupe keeps
  the least restrictive conditionality of equal paths (`digest.rs:3199-3212`), so the overall least is the same with
  or without duplicates.

So each effect keeps instead: the `MAX_PATHS + 1` smallest distinct paths with their conditionality (a bounded
sorted window; a path larger than every kept one is dropped, and an evicted path can never return, since the window
only ever keeps smaller ones), the running least rank, "has an unconditional path", "has a non-`unknown` path", and
the OR of the inputs' truncation. Output is byte-equal to today's merge for any input sequence; test W10 is that
equivalence over every effect of the corpus (old merge kept test-only as the oracle) plus a hand-stated input with
equal-JSON paths of different conditionality. Memory per effect: O(`MAX_PATHS`).

#### 14.3.1b Conditionality over shortest witnesses (A1-03, accepted, operator veto noted)

An effect's conditionality is the least restrictive over its witness paths, as today; the witness paths are now the
shortest ones (plus the proofs). Today, when the old search finished (no cut, no limit), its set was every simple path
that stops at the first carrier, of any length. So if the shortest path is guarded and a longer path is unguarded,
today said `unconditional-on-success`, the DAG says `conditional`. This is accepted as the rule's meaning: a
witness is a shortest path, and conditionality is read over the witnesses. Measured on CDO on today's cut tree: 2 of
138 effects changed conditionality, both `unknown` → `conditional` where the old search hit its state limit
(§14.3.4). Test W7 pins it (a guarded shortest path plus an unguarded longer one → `conditional`). **Operator veto
point:** the sound alternative (a carrier reachable at a depth greater than `dmin` turns the effect's
conditionality to `unknown` unless a witness is unconditional) would mark most dependency effects `unknown`; it is
not taken unless the operator says so.

#### 14.3.2 Removed limit

The BFS half can no longer emit `state-limit-exceeded` or set `limit_hit`: it has no state budget. That diagnostic now
comes only from `enumerate_proofs`. Its CDO count on the cut is part of Part 1 (§14.9).

#### 14.3.3 Determinism

Seeds are sorted; out-edges are pre-sorted; the BFS queue is FIFO; predecessors are appended in that order;
carriers at `dmin` are sorted; the forward walk takes successors in hop-JSON order; a group's resources are a sorted
set. No hash-map iteration order reaches the output. The roots still run in parallel (`digest_query_with`,
`roots.par_iter()`, `digest.rs:3044`), each with its own `HopCache` and `DagCache`.

#### 14.3.4 Output changes, measured on the prototype

Measured on CDO with production `alsem` built from the prototype, on TODAY's cut tree, old search against DAG
(`w6/proto1.sh`, `proto3.sh`; outputs `w6/p1/`, `w6/p3/`). The DAG is the only difference.

| Output | Old | DAG |
|---|--:|--:|
| `d` (cut): effects | 138 | 138 (types, detail, evidence equal) |
| printed via paths | 370 | 336 |
| effects with 3 / 2 / 1 printed paths | 116 / 0 / 22 | 84 / 30 / 24 |
| effects whose printed paths have mixed lengths ("padding") | 36 | 4 |
| `viaPathsTruncated: true` | 88 | 84 |
| conditionality `unknown` → `conditional` | — | 2 effects |
| `o` SARIF (d47/d49/d51), `--scope all` and default | — | byte-identical (1 + 3 rounds) |
| `o` wall (whole command, 3 rounds) | 17.56-17.91 s | 17.01-18.13 s |
| `o` peak | 3,112,528-3,148,344 KiB | 3,131,136-3,151,104 KiB |
| `d` (cut) wall / peak | 21.58-21.99 s / 3,125,852-3,157,336 KiB | 21.25-22.40 s / 3,130,496-3,163,296 KiB |

Reading: the old search filled the 3 printed slots with LONGER paths when fewer than 3 shortest ones existed
("padding"). The DAG prints shortest paths only, so 30 effects now print 2. The 4 effects still mixing lengths merge
facts with different first hops or carriers. The 4 truncation flips and the 2 conditionality flips are expected
where the old search hit its state limit (`incomplete` folds into truncated and makes conditionality `unknown`).
These are intended changes of a Rust-owned output (CLAUDE.md, Testing Philosophy): every printed BFS path is now a
shortest witness. The producer proofs (`enumerate_proofs`) are unchanged and may still be longer.

**`viaPathsTruncated` keeps its public meaning** (A1-08): an input witness set was truncated (the search reached its
cap, or a proof was truncated or incomplete), or more than `MAX_PATHS` (3) distinct paths exist for the effect. What
changes is only the path set it is read over (shortest witnesses), so the flag flips where that set changes.

**The output-change allowlist** (A1-09) for the DAG rule, in any universe — a moved golden or CDO field outside it
is unexplained:
- `viaPaths` (padding gone; a witness the old search lost to its state limit now found) and `viaPathsTruncated`;
- `conditionality`, and the `unknown`/`conditional` reasons that follow from it;
- the diagnostics `state-limit-exceeded` (BFS half: gone), `depth-exceeded` (per fact, item 6),
  `path-limit-reached` (now `cap + 1` shortest), `terminal-not-found`;
- occurrence and fact ids (`factId`, `canonicalKey`, `linkSignature`): they hash the printed via paths
  (`build_canonical_key`, `digest.rs:2780-2820`), so they move with them;
- prove's answer and blockers where they read truncation or conditionality (`prove.rs:405-558`);
- the fingerprint query's witness output.
Moves allowed only for digest's cone-graph tree (§14.3.5): grouped effects, `enters <App>.<Routine>` terminals, the
group `detail`. Never allowed: `o`'s SARIF (A6.22), r4/r4f, `r4_cross_app_program`, effect types/details/evidence on
the cut. P-4 (triage of every changed CDO effect) and P-5 (the corpus goldens) are REQUIRED gates (A1-10): their
results are recorded before Task 2a's commit.

#### 14.3.5 Rule B: boundary witnesses and grouped boundary effects (operator decision, digest's tree only)

The IFDS summary-edge idea (Reps, Horwitz and Sagiv, "Precise interprocedural dataflow analysis via graph
reachability", POPL 1995): a callee's effect is used at the call site as a summary, without re-walking the callee.
Here, applied to the dependency boundary of digest's cone-graph tree:

1. **Boundary witness.** A witness path (item 4 of §14.3.1) ends at the first DEPENDENCY routine it enters (role by
   #220's rule). The kept path is its primary-code prefix; its terminal hop is the boundary call:
   `kind: callsite` at the call site of the edge into the dependency routine, display text
   `enters <Dep App name>.<Routine display>`. An edge into the dependency routine that has no call site (an event
   dispatch or an implicit trigger) gets a synthetic terminal with the same display. Paths inside dependency code
   are never enumerated or printed. Paths that differ only inside dependency code become one path.
2. **Grouping.** An effect whose witnesses all end at boundary terminals (it is reached only through a dependency
   call) is keyed by (effect type, boundary call site) instead of `dedupe_key`'s (type, terminal, detail): all its
   resources become ONE effect. The group's `detail` is
   `{"boundary": "<Dep App name>.<Routine display>", "resourceCount": "<N>", "resources": "<r1>, <r2>, …"}` (detail
   values stay strings, as today): N distinct resources, the first 10 in resource-id order, each its
   `resourceDisplay` when present, else its `resourceId`. `evidence` is the boundary call site; `provenance`
   `transitive`; `viaPaths` the primary prefixes. An effect with a primary-code terminal is unchanged.
3. **Ordering.** The group is ONE effect and so ONE ordering occurrence (its terminal is the boundary call site).
4. **Reads stay out.** Digest reports no reads today (`map_op` has no read op); grouping does not add any.
5. **#161.** On the dependency fixture, `CallChain` reports `DB_MODIFY` and `COMMIT` as two groups through the call
   into `Dep Lib.Outer`, Customer (18) in the `DB_MODIFY` group's resources. T1 asserts exactly that.

Measured on CDO (prototype `w6/proto9.sh`, mimalloc, 3 rounds each; §14.4): `d`'s 15,983 effect facts give 1,020
effects (56 groups, 19,509 printed hops; the shortest-path witnesses alone gave 15,983 effects and 1,230,675 hops);
the 88-root file gives 22,232 effects (862 groups, 411,655 hops).

Tests: B1 (T1's shape above; break: key by `dedupe_key` → one effect per resource → fails); B2 (a primary-terminal
effect beside a boundary group is unchanged; break: group every transitive effect → fails); B3 (more than 10
resources → `resourceCount` exact, 10 listed in order); B4 (the rule is off on the cut: the same fixture
single-app prints today's per-resource effects; break: switch the flag on for the cut → fails).

#### 14.3.6 What a reader of digest sees (the CHANGELOG note)

"Digest (cross-app, the default) now reports what a changed routine does through dependency code: effects reached
through a call into a dependency app are grouped per call and effect type, with the dependency routine named
(`enters <App>.<Routine>`), the number of resources and up to 10 of them. Witness paths stop at the dependency
boundary. Every printed witness path is a shortest one."

#### 14.3.7 The ordering engine's happens-before test, on demand (A1-11)

`compute_ordering` (`ordering_engine.rs:248`) built every happens-before pair of each owner group
(`build_hb_edges`, `ordering.rs:181-205`, all pairs, two `String` clones per edge) and answered each query by a
linear scan of all edges (`has_intra_edge`, `ordering_engine.rs:376-378`, called inside nested loops at `:555` to
`:1430`); it also cloned the whole group's ops into every occurrence (`terminal_routine_ops`, `:394-401`; the field
has no reader outside tests). With thousands of effects sharing an owner that is quadratic in memory and super-
quadratic in time. Fix, byte-equal by construction:
- `has_intra_edge(x, y)` is answered on demand: same group, positions `i` and `j`; for `i < j` it is
  `OB(x,y) && MC(x,y)`; for `i > j` it is `!(OB(y,x) && MC(y,x)) && OB(x,y) && MC(x,y)` — exactly the edge
  `build_hb_edges` emits for that pair (its `else if`). Position lookup through a map built once per call, from an
  occurrence id to ALL its (group, position) pairs: the old scan matched an edge between any two occurrences with
  those ids, so the on-demand test answers true when any pair of positions does.
- each group's ops are built once and shared (`Arc<[OrderedOp]>`) by its occurrences.
Measured (prototype): CDO `o` SARIF byte-identical (`--scope all`, `--scope primary`, `--single-app`; hash
`60708358…`, the parent's); the cut digest JSON byte-identical to the DAG-only build; `d`'s per-root ordering pass
2.5 → 0.2 CPU-s. Test O1: the on-demand test equals `build_hb_edges` for every ordered pair of every corpus root's
groups (break: drop the `!(…)` guard → fails on a pair where both directions hold).

### 14.4 Cost bounds for `d` (replace B1, B2 and S1/S2 for `d`)

`d` cannot stay within parent + band: on the cone graph its roots reach every write their dependency callees make,
and reporting them is #161's point. So digest gets a STATED cost (operator decision 1), for `d` and, since a
digest of a whole file is the ordinary case (A1-12), for a multi-root file:

- `d` = cross-app `digest <WS> --file src/DocumentTypes/Finance/General/CDOIssueDocument.Codeunit.al` (3 roots);
- `h` = cross-app `digest <WS> --file src/Features/HandledState/CDOUnhandledSalesPagesMgt.Codeunit.al` (88 roots).

Measured on the rule-B prototype (`w6/proto9.sh`, output `w6/p9/`; the probe under mimalloc with purge delay 0,
§14.6; 3 rounds each; ms and KiB). Parent terms are from Task 1 and are re-measured under the same rule at Task 1′.

| Term | `d`, 3 rounds | `d` median | `h`, 3 rounds | `h` median |
|---|---|--:|---|--:|
| K (inputs + compose) | 6,132.8 / 6,012.1 / 5,885.9 | 6,012.1 | 6,212.4 / 6,932.8 / 6,351.9 | 6,351.9 |
| T (`compose_tree`) | 2,321.0 / 2,243.3 / 2,228.1 | 2,243.3 | 2,530.2 / 2,617.4 / 2,589.4 | 2,589.4 |
| E′ (S4 entries + unresolved cones) | 3,162.0 / 3,058.6 / 3,080.6 | 3,080.6 | 22,004.8 / 21,203.5 / 21,251.4 | 21,251.4 |
| probe process peak | 3,100,628 / 3,116,448 / 3,098,464 | 3,100,628 | 3,115,068 / 3,104,576 / 3,122,936 | 3,115,068 |
| effects (groups) | 1,020 (56) | | 22,232 (862) | |
| digest's ordering pass, CPU-s | 0.2 | | 111.8-124.8 | |
| D_P, S_P(d) (Task 1) | 6,894.9, 2,901.4 | | — | |

`S1′(d) = K − D_P + T + E′ − S_P = 6,012.1 − 6,894.9 + 2,243.3 + 3,080.6 − 2,901.4 = 1,539.7 ms` predicted extra
wall for `d`; the probe peak is the parent's whole-command peak (3,128,520 KiB) within noise.

**The bounds** (pre-stated; the operator's numbers):

| Bound | Rule | Measured | Void check |
|---|---|---|---|
| **S1′** (Task 1′ stop rule, probe) | `d`: `K − D_P + T + E′ − S_P ≤ 8,000 ms` | 1,539.7 ms | probe rounds ≥ 3 in the quiet slot |
| **S2′** (Task 1′ stop rule, probe) | `d`: probe peak `≤ Pm(d) + 3,145,728 KiB` (3.0 GiB) | ≈ Pm(d) | none |
| **S3′** (Task 1′ stop rule, probe) | `h`: E′ `≤ 60,000 ms` and probe peak `≤ 12,582,912 KiB` (12 GiB) | 21,251.4 ms; 3,115,068 KiB | probe rounds ≥ 3 |
| **B1′** (A6.3 wall, final bench) | `Hd ≤ Pd + 8,000 ms + Bd` | — | `Bd ≤ 0.05 × Pd`, no load tick > 8 (§8.1) |
| **B2′** (A6.3 peak, final bench) | `Hm(d) ≤ Pm(d) + 3,145,728 KiB` | — | none |
| **B6** (multi-root wall, final bench) | `Hh ≤ Ph + 60,000 ms + Bh` | — | `Bh ≤ 0.05 × Ph`, load as B1′ |
| **B7** (multi-root peak, final bench) | `Hm(h) ≤ 12,582,912 KiB` | — | none |

The `h` numbers set no tighter bound than the operator's: 12 GiB keeps four parallel `alsem` jobs inside the 32 GB
tentacle limit with room for the OS, and 60 s is the slowest digest a review hook should wait for. `Pd`, `Pm`, `Ph`
and every term are re-measured at Task 1′ on the branch's base; the bound constants are fixed here. A later saving
does not tighten them, and a miss is never relabelled. The post-attach peak stays "not measured" (6A-8).

### 14.5 `o` and policy under the split

- **`o` (A6.4) is a no-regression bound again**, as row 6 words it: B3 `Ho ≤ Po + Bo` on `analyze.total`
  (void check `Bo ≤ 0.05 × Po`), B4 `Hm ≤ 1.03 × Pm`. The head changes `o`'s path only by the DAG rule; measured
  neutral (§14.3.4). E15's "6 within parent" holds for `o`.
- **Policy (Task 3) is kept only if B5 holds**: cross-app `policy check` kernel peak `≤ 1.03 × Pm(policy)` (Task 1
  parent: 14,324,904 KiB → 14,754,651 KiB). Task 1's early warning did not fire (probe policy composition 9,433,852
  KiB < 14,324,904 KiB), so F13 is not pre-decided. Policy has no tree, so the DAG does not touch it. If B5 misses,
  F13 runs once (§10); if it still misses, **Task 3's commit is reverted on the branch before the merge**
  (`git revert`, no force), `RAW_INHERITED_FACTS`, policy's RAW build and `inherited_raw` stay, and a follow-up is
  filed. Task 2 does not depend on Task 3.
- **The rollback is independent of Task 2b** (A1-13). Task 2b turns `build_detector_artifacts` into an assembler
  that consumes `caps` and composes nothing; today's policy RAW build calls it to compose (`pipeline.rs:255-272`).
  So Task 2b keeps policy's RAW build working by giving it a command-local composing builder (the old body, over the
  same `det_cone_inputs`, `ConeScope { derived: true, raw: All }`), used only by policy until Task 3 deletes it.
  Tests: Task 2b's commit alone passes the policy goldens and the CDO `policy check` `cmp`; the state after a revert
  of Task 3 is checked the same way before the merge if the revert fires.

### 14.6 Memory plan

**Measurement rule (A1-14).** Every peak that a bound reads is taken under production `alsem`'s allocator: on
`alsem` itself, or on a probe binary that installs mimalloc and runs with `MIMALLOC_PURGE_DELAY=0` (`alsem.rs:28`,
`:31-63`). The first §14 text took its probe peaks on `aldump` under the system allocator, which inflated them
(same `d` run: 6.0 GB system malloc, 4.79 GB mimalloc; `MALLOC_ARENA_MAX=2` alone gave 4.71 GB).

Where the memory went (`d`, mimalloc, `w6/p7/`, `w6/p9/`, KiB):

| Configuration | Peak |
|---|--:|
| K + T only (effects skipped) | 3,117,124-3,126,384 |
| shortest-path DAG witnesses, no boundary | 4,784,684-4,793,032 |
| same, one thread | 3,996,264 |
| same, at most 3 paths per fact | 4,739,788 |
| rule A alone (boundary witnesses, no grouping) | 28,269,536-29,036,648 |
| **rule B + on-demand happens-before (the design)** | **3,098,464-3,116,448** |

Attribution: not the DAGs (4 builds, 76,897 nodes, 95,061 predecessors for `d`), not the number of paths per fact
(3 instead of 256 changed nothing), but the number of EFFECTS per root and digest's per-root ordering pass over them
(§14.3.7), times the roots running in parallel. Rule B bounds the effects; §14.3.7 removes the quadratic. The merge's
retained paths are bounded too (§14.3.1a). P-2 (a census arm, `digest.effects` and `digest.ordering` buckets per
root) confirms this at Task 1′ on the port; a peak more than 10 % off the prototype's is a finding, as for time.

### 14.7 What is void, kept or changed

| Item | Status |
|---|---|
| 6A-6 (ordering subjects = reportable roots) | void: the ordering plan has no witness scope |
| 6A-10 (`o`'s cost bound), Q1, Q9 | void: `o` is no-regression again (§14.5) |
| 6A-1, 6A-2, 6A-3, 6A-4, 6A-5, 6A-7, 6A-8, 6A-9, 6A-11 | kept. 6A-7's void check now applies to B1′ and B3 |
| §0 item 1, the feasibility rows A6.3/A6.4 | replaced by §14.4 and §14.5 |
| S1, S2 for `d` (`≤ Bd`, `≤ 1.03 ×`) | void, replaced by S1′, S2′ |
| S1, S2 for `o` | void |
| B1, B2 | replaced by B1′, B2′ |
| B3, B4 | kept as no-regression bounds (§8.1 table, without 6A-10's rule) |
| B5 | kept; now also decides whether Task 3 stays (§14.5) |
| F1, F3 | as before (withdrawn, promoted) |
| F2 (ordering ingredient gate) | void: it was `o`'s first rung |
| F4 | kept, dormant; its reader is now digest's per-root ordering pass only |
| F5, F6, F7, F9 | kept |
| F8 (> 30 % FP on new d47/d49/d51 findings) | void for step 6: no d47/d49/d51 finding may move (A6.22); a move stops the claim (F-W3) |
| F10 | replaced by F-W1/F-W2 for B1′/B2′/B6/B7. For B3/B4 (`o`): a miss STOPS the claim and goes to a re-panel (A1-15); F11 touches only digest's composition and cannot fix `o` |
| T8, A6.6 (no raw for a dependency subject) | read on `d`, the multi-root `h` and policy only; `o` keeps the cut's `{false, All}` composition, a stated I5 exception (§14.2, A1-06) |
| W-tests | W1-W6 as first written, plus W6b, W7, W8, W9, W10, B1-B4, O1 (§14.3) |
| §4.7 `Cell::Effects` contents | each root keeps its S4 entry, its unresolved-cone items AND their `UnresolvedTraversal` (depth, truncation, visited routines; serialized at `digest_cli.rs:1230-1246`, read by prove's obligations at `prove.rs:207-224`), plus `identities` and `coverage` (A1-16). A test pins the serialized traversal block of a cli-b digest golden after the tree is released |
| F11 (#228, reuse the base's direct maps) | kept, now only as a K reducer for `d` |
| F12 (`dependency_calls` > 50 MB) | void unless P8 changes: 40,771,782 B measured |
| F13 | kept for Task 3 |
| §4.8 rows "the cut graph" and "ordering's own snapshot" | struck: both stay |
| §4.4 "`compose_snapshot(model)` asserts no dependency-role row" | struck: the ordering cell calls it cross-app |
| §4.4 `compose_full_snapshot` cross-app over `rel.cone()` | deferred to step 9 with its readers (6A-9): it keeps today's cut base |
| T3's ordering arm, T3 break (3) on the ordering cell | moved to the #161 follow-up. Break (3) stays as evidence on the digest arm |
| T1 | digest half only |
| A6.11 (ordering root-set oracle) | moved to the #161 follow-up |

### 14.8 Fallbacks for the split

| # | If | Then |
|---|---|---|
| F-W1 | S2′, S3′'s peak, B2′ or B7 misses | name the owner with P-2's census (`digest.effects`, `digest.ordering`, DAG, merge window). ONE fix aimed at that owner (A1-12): cap the parallel roots of digest's effects step to the CPU count ÷ 2 if the owner is per-root and the roots overlap; or the compact DAG (dense `u32` ordinals, one flat predecessor `Vec`) if the DAG owns it. Re-run W-tests, identity gates (golden targets, A6.22) and Part 2 once. Still missed → **D** (below) |
| F-W2 | S1′, S3′'s E′, B1′ or B6 misses (met-not-void) | name the span (`trace_summary.py`); ONE fix from F11 or F-W1's; re-run Part 2 once; still missed → **D** |
| F-W3 | any d47/d49/d51 finding moves (CDO Part 1, r4, r4f, `r4_cross_app_program`) | stop and ask the operator (A1-26): a move may be a real improvement of a shortest witness, but A6.22 forbids it in this step without a decision |
| **D** (pre-stated, automatic) | F-W1 or F-W2 failed, or S1′/S2′/S3′ fail at Task 1′ | step 6 keeps Task 1's groundwork (`edca9f0c`) and Task 3 (policy, if B5 holds); **digest stays on the cut**: Task 2b is not merged, Task 2a (the DAG rule, rule B's code paths off, the on-demand happens-before test) stays only if its own gates are green; the digest-on-cone-graph switch is filed as a follow-up with the numbers. No bound is relabelled |

### 14.9 Acceptance matrix and measurement plan deltas

Acceptance (§6):

| # | Change |
|---|---|
| A6.1, A6.9 | T1, digest half, in rule B's grouped shape (§14.3.5); the d47 half → #161 follow-up |
| A6.3 | **B1′, B2′** (cost bounds), plus **B6, B7** for the multi-root file; feasible on the prototype's numbers (§14.4) |
| A6.6 | T8 on `d`, `h` and policy; `o`'s cut composition is the stated I5 exception (§14.2) |
| A6.4 | **B3, B4 no-regression**, feasible (measured neutral) |
| A6.10 | digest arm of T3 only |
| A6.11 | → #161 follow-up |
| A6.15 | `ConeOutput`, the timings mode and (with Task 3) `RAW_INHERITED_FACTS` deleted; `build_r3a3_source_only_base` and the cut text STAY, the text rewritten to name the ordering cell and #161 |
| A6.19 | digest goldens may move by the cone graph (cross-app, `cli_digest_query_dependencies`) AND by the DAG rule (any universe); prove and fingerprint witness goldens may move by the DAG rule only. Each move triaged and attributed (cone graph / DAG / D6). `r4_cross_app_program`, r4, r4f must not move |
| **A6.21 (new)** | the DAG rule, the streaming merge and the on-demand happens-before test: W1-W6, W6b, W7-W10, O1 (plan Task 2a) with discrimination proofs; rule B: B1-B4 (plan Task 2b) |
| **A6.23 (new)** | the output-change allowlist (§14.3.4): every moved golden and CDO field is inside it; P-4 and P-5 recorded |
| **A6.22 (new)** | d47/d49/d51 byte-identical: CDO SARIF `cmp` cross-app at `--scope primary` and `--scope all`, and `--single-app`; r4/r4f/`r4_cross_app_program` unmoved |
| A6.7, A6.12 | Task 3 only if B5 holds (§14.5) |

Measurement plan (§8):
- `P` = the master the branch sits on (`6a2b7c8b`, or a later master after a rebase), release-fast, never
  `7afa91a3`.
- **Task 1′ (claim time, before Task 2a):** the probe at `P` with the DAG, rule B and the on-demand happens-before
  test, under mimalloc (§14.6), on `d` and `h` (`probes-todo.md` P-1, P-2 with its census arm), read S1′/S2′/S3′.
  P-4 (triage of every changed CDO effect) and P-5 (corpus golden diff) are required and recorded before Task 2a's
  commit. The E′ outlier rule (A1-25): a probe round whose load tick exceeds 8 is void and re-run; no round is
  excluded by hand.
- **Part 1** adds: `--single-app` digest JSON parent vs head = the DAG rule's effect alone (no cone-graph change
  single-app, P13); cross-app `d` JSON diff, each moved field attributed to cone graph, DAG or D6; the
  `state-limit-exceeded` count in `d` (cut parent vs head); A6.22's `cmp`s. Digest JSON is compared on `P` ≥
  `6b09105b`, so #78's `objectDisplay` is on both sides.
- **Part 2:** `d` read against B1′/B2′, `h` against B6/B7, `o` against B3/B4, every peak on `alsem` (mimalloc).
  Round rule unchanged (§8.1).
- Part 1 also times the fingerprint query on CDO single-app (`fingerprint <WS> --query …`, parent vs head), reported
  (A1-05).
- **Part 3:** B5 decides Task 3 (§14.5).
- Census: one new line `digest.witness_dags=N nodes=N preds=N` per run (P-2's census, kept as a counter).

### 14.10 Counters (§4.9 rows that change)

| Row | `bcg` | `cone` | `v2` | `scc` | `pd` | `ctx` | `snap` | `witness` |
|---|--:|--:|--:|--:|--:|--:|--:|--:|
| analyze cross-app / single-app d47 | 2 (unchanged, P9) | 1 | 0 | 2 | 1 | 1 | 1 | 1 (the cut's `{false, All}` composition) |
| digest cross-app / single-app | as §4.9 (1/1/1/2/1/1/1/1) | | | | | | | |
| policy | as §4.9 if Task 3 stays; else today's (P9) | | | | | | | 1 / 1 |

| analyze with d47 AND a summaries detector (A1-18) | 2 | 2 | 1 | 3 | 2 | 1 | 1 | 1 (the cut's) |

The digest row assumes digest's plan wants no `ORDERING_FACTS` (true for the default detectors: d47/d49/d51 are
opt-in). A digest run with an ordering detector would add the cut composition: stated, not tested. The two-composition
analyze row is pinned by T5.

### 14.11 Interactions with #78 (`6b09105b`, owner names)

- `resolve_changed_roots` now takes `owners: &OwnerIndex<'_>` (`digest_cli.rs:375-381`) and narrows
  `<owner>::<routine>` selectors with it. D3's change (`&CapabilitySnapshot` → `&SnapshotIdentityTable`) composes
  with it: `build_selector_indexes` still reads `snap.identities` only (`selector_index.rs:84`, P5 holds).
- `OwnerIndex<'a>` borrows the model. Digest resolves its roots BEFORE `prepare` composes (D3), so it builds the
  index in a block that ends before `prepare(&mut view)`, and builds it again from `view.model()` for the query at
  (8). It is one pass over routines and objects (`effect_query_cli.rs:470-506`), so two builds cost little; P-1 checks.
  The Task 1 probe (`step6_probe.rs`) builds it the same way since `fix(#160)` on the branch; its `--file` roots never
  read it.
- `run_digest_query_full_from_entries` now takes `changed: &ChangedRootsResult` and `owners`
  (`digest_cli.rs:768-776`). Task 2b's switch to `DigestEffects` replaces only the `snap` argument.
- `objectDisplay` comes from the model, never from the identity table, and only for roots, which are primary. The
  cone graph adds dependency routines to the tree but never as roots, so #78's output cannot move from the switch.
  `cli_object_display.rs` (A1-A7) runs digest in the default (cross-app) mode: after Task 2b it runs over
  `compose_tree`, so it is a free guard for the switch. `assert_identity_table_holds_extension_name` keeps working:
  the table comes from the same `derive_identity_table(model)`.
- Goldens: #78 moved 41 digest, 46 fingerprint and 36 prove goldens (`objectDisplay`, headers). Task 2's triage
  starts from the post-#78 goldens; a moved `objectDisplay` or `<owner>::` header in Task 2a or 2b is unexplained.

### 14.12 Citations that moved after `7afa91a3`

§0-§13 cite master `7afa91a3`. Of their files, only these changed up to `6a2b7c8b`
(`git diff --stat 7afa91a3 6a2b7c8b`): `digest_cli.rs`, `snapshot_full.rs`, `fingerprint_cli.rs`, `prove.rs`,
`selector_index.rs`, `capability_cone.rs`, `exec_relation.rs`. `digest.rs`, `snapshot.rs`, `detector_context.rs`,
`session.rs`, `registry.rs`, `pipeline.rs` did not move. Each row below was checked by matching the cited line's text
in both trees.

| Spec cite (at `7afa91a3`) | At `6a2b7c8b` |
|---|---|
| `digest_cli.rs:372` `resolve_changed_roots` | `:375` (+ `owners` parameter) |
| `digest_cli.rs:381`, `:386` span index / selector build | `:385`, `:390` |
| `digest_cli.rs:398-423`, `:436-510` | `:402-430`, `:443-517` |
| `digest_cli.rs:766`, `:779`, `:789-800`, `:990` | `:768`, `:782`, `:792-803`, `:996` |
| `digest_cli.rs:1632-1639`, `:1655`, `:1682` | `:1634-1641`, `:1663`, `:1690` |
| `snapshot_full.rs:888` `derive_permissions` | `:892` (`:137-189`, `:163`, `:184`, `:507` unchanged) |
| `fingerprint_cli.rs:437`, `:498` | `:438`, `:499` |
| `prove.rs:890` | `:887` |
| `selector_index.rs:84`, `:112` | unchanged (the file grew after line 198 only) |
| `capability_cone.rs` (#221 removed salsa derives) | lines 133-3670 move up 3 (`:2580` → `:2577`, `:2586` → `:2583`, `:2641` → `:2638`, `:2650` → `:2647`, `:1667` → `:1664`); lines 3674-4143 up 5 (`:4072` → `:4067`); from 4148 up 8 (`:4485` → `:4477`, `:4514` → `:4506`, `:4600` → `:4592`, `:4614` → `:4606`); lines before 129 unchanged. On the branch, Task 1 (`edca9f0c`) moves them again: re-cite on the branch file when a task edits it |
| `exec_relation.rs:50`, `:69-93`, `:111-114` | unchanged (the file changed after line 400 only) |

### 14.13 Decisions (operator, 2026-10-23)

All four were decided as recommended at the attempt-8 resume:

1. **A stated cost for `d` is accepted** (B1′: +8.0 s; B2′: +3.0 GiB), an explicit change to E15's "6 within
   parent" for digest. Reason: the cost is the dependency effects #161 asks for.
2. **One witness rule everywhere** (digest, prove, fingerprint query, ordering). Rust-owned digest, prove and
   fingerprint witness goldens may move (padding, truncation); d47/d49/d51 must stay byte-identical (A6.22).
3. **D6 on the cone tree only**; the cut keeps today's ledger until the #161 follow-up. It is what Task 1 built and
   it keeps d47/d49/d51 identical by construction.
4. **`compose_full_snapshot` cross-app stays on the cut base until step 9** (it is test-only cross-app).
5. **Digest's output for dependency effects is rule B** (after the §14 panel round 1): witnesses stop at the first
   dependency routine; effects reached only through a dependency call are grouped per (boundary call site, effect
   type) with a resource count and up to 10 resources. Grouping per resource (a finer opt-in) and full
   in-dependency witnesses are possible later opt-ins, not in this step. Digest keeps reporting no reads.
6. **Bounds** (§14.4): `d` E′ within +8 s and peak within +3.0 GiB; the multi-root file within 60 s E′ and 12 GiB
   peak; d47/d49/d51 byte-identical. A miss after its one fix applies **D** automatically (§14.8): digest stays on
   the cut.
7. **Conditionality over shortest witnesses** (§14.3.1b) is accepted as recommended; the operator may veto it in
   favour of the `unknown`-when-a-deeper-carrier-exists rule.
