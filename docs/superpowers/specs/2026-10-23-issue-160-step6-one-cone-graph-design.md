# #160 child spec: step 6 — one cone graph, one scoped witness pass

Status: spec panel round 1 fixes applied (2026-10-23; findings R1-01 to R1-33 in
`.agent/issue-160/findings.json`, §0.1). Made from the pre-draft
`/work/bench/160-6/draft-step6-design.md` (master `5c42b8f3`) once the base probes had run;
§0 lists what the probes changed, §0.1 what round 1 changed.

Child of `2026-10-18-issue-160-analysis-session-design.md` (the epic): §1 goals 2, 6; §5.5
(M3, M5, M8), §5.6, §5.7, §5.9 rule 3, §5.10, §5.12; §9 row **6**; E1, E3, E9, E12, E15, E18;
I1-I10; Decisions 2, 8, 14, 15, 16, 18, 20, 22-24, 33, 41-43, 47, 50-52, 56, 58; carried
findings G-1, G-2, G-4, R6-4; open questions 12, 13, 14. The shape follows
`2026-10-22-issue-160-step5-coverage-policy-design.md`. Where this document and the epic
disagree, this document is wrong, EXCEPT the numbered amendments of §9 (6A-1 to 6A-11). Each
gives its reason; until the epic takes it, it is an open question for the panel.

Base for the citations: master `459294f1` (step 5's merge, #223), worktree
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
  (`capability_cone.rs:4505-4513`), which cross-app holds primary callers only ("legacy cut",
  epic M9; there is no `LegacyCut` symbol, only that doc text). **Cone graph** = `rel.cone()`
  (`exec_relation.rs:111-114`): the primary half's typed edges with event dispatch, then the
  dependency half's.
- **Raw** = a routine's `Vec<CapabilityFact>` of inherited facts. **Derived** = its
  `ConeDerivedStore` row. **pd** = the producer solve's `ConeArena`
  (`capability_cone.rs:2640`).
- **Tree** = the view's `CapabilitySnapshot` (`snapshot.rs:881`) **[#220]**. **Effects** = the
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
4. **The cone is still keyed by string id after 11b** (`capability_cone.rs:2585`), so
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
   (`build_r3a3_source_only_base`, `_over`, `R3a3SourceBase`, `capability_cone.rs:4476-4609`)
   and the measurement-only `aldump --issue160-full-graph-timings` (`:4611-4680`,
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
  `capability_cone.rs:4063-4131`) keeps composing raw for every routine of whatever workspace
  it is given, through `WitnessScope::All`; it is a diagnostic tool for fixtures, stated here as
  the one exception. `project_r3a3` likewise.
- **Discoveries deferred** (§12): #215, #216, #217, #227, #228 (kept as fallback F11).

## 3. Background (at `459294f1`)

### 3.1 The compositions today

| Command (cross-app) | det cone (`detector_context.rs:1387-1394`, over `rel.cone()`) | snapshot cone (`capability_cone.rs:4582-4591`, over the cut) | counters `cone`/`pd`/`scc` (`tests/cli/cli_build_counters.rs`) |
|---|---|---|---|
| analyze default | `DerivedOnly`, every routine | none | 1/1/2 (`:192`) |
| analyze `--detector d47` | none (no `SUMMARIES`/`TRANSACTION_SPANS`, `:1321`) | `RawOnly`, every routine, from `fill_ordering` (`detector_context.rs:1114-1121` → `digest.rs:4016-4019`) | 1/1/2 (`:233`) |
| digest | `DerivedOnly` | `RawOnly`, after `det` is released (`digest_cli.rs:1632-1639`) | 2/2/3 (`:335`) |
| prove (single-app) | `DerivedOnly` | `RawOnly` (`prove.rs:890`) | 2/2/3 (`:389`) |
| policy check | `DerivedOnly` (the run) + `Both`, EVERY routine incl. dependency rows (`gate/policy/pipeline.rs:266-272`) | none | 2/2/3 (`:624`) |
| fingerprint (single-app) | `DerivedOnly` | `RawOnly` (`fingerprint_cli.rs:497`; json path through `compose_full_snapshot`, `:436` → `snapshot_full.rs:84`) | 2/2/3 (`:444`) |

Both compositions call `compose_inherited_cones` (`capability_cone.rs:2579`). Its
`routine_ids` is both the emission set and the raw-walk set (`:2819-2822`); its `mode` gates
the raw walk (`:2836`, recursive SCCs) and the singleton's raw half
(`inherited_facts_for_singleton`, `:2885-2898`). The fact cones (`fact_cone_for_scc`,
`:2713-2715`) and coverage cones (`:2729-2742`) are built for every non-root SCC whatever the
mode, so a per-member raw gate changes no other routine's output. Every member's result goes
into one `out` map returned at the end (`:2938-2955`): all raw is alive together when the
composition returns.

### 3.2 What the tree's readers read

- digest S4 entries (`compute_digest_effects_cli`, `digest.rs:4094`): the root's own facts
  (`facts_by_routine.get(rid)`, `:3260`), every routine's DIRECT facts (`:243-315`), typed
  edges, the pd arena. Direct facts stay per routine of the view; only raw is scoped.
- the digest query (`run_digest_query_full_from_entries`, `digest_cli.rs:766`): `coverage`
  (`:779`), `identities` (`:789-800`) and `unresolved_cone(snap, rid)` (`:990`), a BFS over
  typed edges that collects every visited routine's `callsite_index` and
  `callsite_resolutions` rows (`unresolved_cone.rs:94-140`). Cross-app it walks into
  dependency routines, so it is the reader of dependency callers' ledger rows (D6).
- the ordering filter (`digest.rs:4035-4061`): `map_op` over every fact; a root is kept on its
  own facts, and the root is a subject.
- the ordering engine (`ordering_engine.rs:170-215`): `operation_index`, `callsite_index`,
  `callsite_resolutions` read only for the ROOT (`resolutions_from(routine_id)`, `:1028`),
  typed edges, and two maps built over every fact (`http_method_by_callsite_id`,
  `background_kickoff_callsite_ids`, `:202-214`). **Neither map can be read through a
  non-subject's fact** (Q3, decided statically, R1-20): the http lookup runs only when the IO
  chain's terminal routine is the root (`:1323-1333`), the background lookup only for the
  root's own call sites (`:1028-1050`), and an inherited fact's `witness_callsite_id` is its
  OWN subject's first hop (`retag`, `capability_cone.rs:1666-1672`). So a non-subject's
  inherited fact carries a non-subject's call site, never a root's. T3 keeps a break as
  evidence; F4 is dormant.
- root resolution (`resolve_changed_roots`, `digest_cli.rs:372`): `build_selector_indexes`
  (`selector_index.rs:84`) reads `snap.identities` and nothing else, and keeps primary ids
  only (`:114`); file and diff matching use `build_routine_span_index(model)` (P5).
- policy (`select_facts`, `policy_engine.rs:110-125`): `inherited_raw()` of primary routines
  only (`pipeline.rs:276-285`).
- the full snapshot (`compose_full_snapshot`, `snapshot_full.rs:80-95`) **[#220]**: the core
  plus `derive_permissions(resolved, &base)` (`:651`), which reads `base.cones[r].inherited` and
  `base.direct_full` for primary routines.

### 3.3 The pd arena

`derive_pd_arena` (`snapshot.rs:1701-1787`) **[#220]** maps every cone state into the tree:
`EdgeIx` into the tree's sorted `typed_edges`, `producer_fact` into its `capability_facts`
(`expect`ed, `:1745-1747`). A cone link whose edge has no tree row is dropped and counted
(`PD_EDGE_UNMAPPED`, `:1764-1769`; a `debug_assert!(false)` in debug).

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
is passed today (`:2836`, `:2876`, `:2885-2898`). Mode `(false, false)` (a non-subject of a
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
(`:1408-1445`) is unchanged except its source. The resolved own entries (`capability_cone.rs:2649`)
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
| analyze with `ORDERING_FACTS` (d47/d49/d51) | the internal ids of the rows of `reportable_roots(model)` (`digest.rs:3698-3727` returns stable ids; mapped back through `stable_routine_id`, every row of a collided id) | yes |
| digest | the changed roots, resolved before `prepare` (below) | yes |
| policy check | every `Role::Primary` routine (#220's rule; today `is_primary_routine`, `pipeline.rs:276-285`) | no |

Digest builds `derive_identity_table(model)` (`snapshot.rs:1372`, model only) before `prepare`,
resolves its roots from it (`resolve_changed_roots` takes `&SnapshotIdentityTable` instead of
`&CapabilitySnapshot`, P5) and passes them in the plan; the table moves into the tree. It is a
command-local, not a View cell, until step 12 (6A-11). A digest with no resolved root keeps
`WitnessScope::None`, `needs_tree: false`, and still exits 2 (`digest_cli.rs:1682`).

Ordering subjects are every reportable root (6A-6); F2 gates them by ingredient if Task 1 says
the raw is over budget.

### 4.4 D4: the tree, composed from `caps`, `raw` and `rel` (replaces `R3a3SourceBase`)

`compose_snapshot_from_base(model, &R3a3SourceBase)` (`snapshot.rs:1250-1290`) **[#220]**
becomes `compose_tree(model, typed: &[TypedEdge], dependency_calls: &[CallEdge], caps:
&mut CapabilityAnalysis, raw: HashMap<..>) -> CapabilitySnapshot`. `R3a3SourceBase` becomes a
struct of BORROWS with the same field roles, so each deriver keeps its body:

| Deriver (`snapshot.rs` **[#220]**) | Today reads | After |
|---|---|---|
| `derive_identity_table` (`:1372`) | model | digest's table (moved in) or a fresh one |
| `derive_capability_facts` (`:1443-1486`) | `base.direct_full` + `base.cones[*].inherited` (cloned) | `caps.direct_full` (borrowed) + `raw` **by move**: each subject's `Vec` is drained into snapshot facts and dropped as it goes, so raw exists once (Decision 24, R1-01) |
| `derive_typed_edges` (`:1501`) | `base.graph.typed_edges` (cut) | `typed` = `rel.cone()` |
| `derive_operation_evidence`, `derive_callsite_evidence` (`:1842`, `:1968`) | `base.ws_routines` (a clone, `capability_cone.rs:4600`) | `&model.workspace.routines`, borrowed |
| `derive_callsite_resolutions` (`:2057`) | `base.calls` (cut, a clone `:4524`) | `model.calls.edges` then `dependency_calls` (D6) |
| `derive_coverage` (`:2442`) | the snapshot's own fold over the cut (`:4544`) | `caps.coverage`: step 5's fold over `rel.uncertainty_all()` (closes 5-A3) |
| `derive_event_declarations`, `derive_root_classifications`, `derive_routine_order_frames` | base | model (`model.events.graph`, borrowed) |
| `derive_pd_arena` (`:1701`) | `base.pd_solution` | `caps.pd.take()`, scoped (D5); the cone arena is dropped as soon as the tree's arena is built (R1-27) |
| `derive_analysis_gaps` (`:2359`) | base | as #220 leaves it (its role rule, `rel.cone()` reach) |
| `derive_permissions` (`snapshot_full.rs:651`) **[#220]** | `base.cones[r].inherited` + `base.direct_full`, primary routines | the tree's inputs, same routines |

Deleted: `build_r3a3_source_only_base`, `_over`, the cut's clone of calls and event graph
(`capability_cone.rs:4523-4526`) and its own fold (`:4542-4575`), the timings mode, and the
"legacy cut" doc text (`:4501-4504`, `combined_graph.rs:309`).

**`compose_snapshot(model)` stays as the single-app convenience** for prove, fingerprint,
diff and tests (`digest.rs:3969`, `:3982`; `snapshot.rs:2713`). It asserts the model has no
dependency-role row (so the cut cannot come back under a new name, R1-09), builds the graph
with `build_combined_graph(&model.workspace, &model.calls, &model.events.graph)` as today
(counted `bcg`; no relation, so no extra Tarjan, R1-08), then `det_cone_inputs` over that graph,
one composition `{derived: false, raw: All}` and `compose_tree`. Single-app the graph's typed
edges are `rel.cone()`'s (P13), so its output is today's snapshot and its goldens do not move.

**`compose_full_snapshot`** (`snapshot_full.rs:80-95`) **[#220]**: single-app, through the
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

**R6-4, the link rule.** `derive_typed_edges` skips a row that has no anchor (any kind,
`snapshot.rs:1558-1600`); it records the keys it skipped. A cone link into a skipped row is
dropped and counted `PD_EDGE_UNMAPPED`, as in release today. The `debug_assert!(false)`
(`:1767`) becomes `debug_assert!(skipped.contains(&key))`: a link may miss only a row that was
skipped for want of an anchor (R1-28). T6 hand-states one; the CDO gate asserts
`PD_EDGE_UNMAPPED == 0` on `d` and `o` (F6 if not).

### 4.6 D6: the ledger reads every caller (Decision 18, M8; Decision 8 ledger half)

`ExecRelation::new` (`exec_relation.rs:69-93`) **[#220]** keeps `dependency_calls:
Vec<CallEdge>` (the `dependency` argument, today dropped after assembly), read only by
`derive_callsite_resolutions`, printed as a `rel.dependency_calls` census bucket. Its reader is
`unresolved_cone` (digest; §3.2), which walks into dependency routines: without these rows the
leftover-typed-edge pass (`snapshot.rs:2211-2255`) still writes a row for each dependency call
site, but always as `status: "resolved"`, `dispatch_kind: "direct"`, no
`result_consumed`/`under_asserterror`, so a dependency routine's unresolved or builtin call
would be misreported (R1-11). It lives in every cross-app view; its bytes are measured at Task
1 (P8) and F12 applies if they are large (R1-29).

In `classify_resolution` (`snapshot.rs:2267`) **[#220]**, a `"resolved"` call whose every
target is a body-less dependency row is `unfetched-dependency` with its edges (today only
`"opaque"` is, `:2339`). **This moves the ordering engine on its own**: such a ROOT call site
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
| `ConeOutput` | `cone_derived.rs:244-263`; call sites `capability_cone.rs:4118`, `:4590`, `detector_context.rs:1375-1379`, tests → `ConeScope` |
| det's own cone | `detector_context.rs:1386-1406` (the call; the summary assembly stays) |
| `RAW_INHERITED_FACTS` | `registry.rs:32-90`, `session.rs:193`, `:273-276` |
| `inherited_raw` and the summary's raw `Option` | **narrowed to `#[cfg(test)]`** (`full_summary.rs:42-130`): production `FullRoutineSummary::new` has no raw parameter; `test_support::summary` (`test_support.rs:240-249`) and `cone_store_of` (`:30-55`) keep raw as fixture input; the raw oracles `may_commit`, `touches_db_of`, `touches_physical_table_of` (`capability_query.rs:62-102`, already `#[cfg(test)]`) keep checking the derived predicates (R1-10) |
| policy's RAW build | `gate/policy/pipeline.rs:255-272` |
| the cut graph | `capability_cone.rs:4476-4609`; `R3a3SourceBase` becomes the borrowed input struct |
| the full-graph timings mode | `capability_cone.rs:4611-4680`, `aldump.rs:61-70`, `:223`, and its smoke test `tests/cli/aldump_smoke.rs:100` |
| ordering's own snapshot | `digest.rs:4016-4019` (`compute_digest_effects_for_ordering_with` takes the tree) |

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

The census is shallow (lower bounds: owned strings are not walked, `snapshot.rs:1309-1333`), so
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
  fact-cone build for non-subject SCCs (`capability_cone.rs:2713-2715`) → a subject loses
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
- **T8 (I5, coverage equality)**: `witness.non_primary_members == 0` on CDO `d`, `o` and policy
  (counted at the gate, by #220's `routine_role`); on the dependency fixture
  `tree.coverage[r] == det.summaries[r].coverage` for every routine. Break 1: set policy's
  production scope to `All` → the materialized count is > 0 → fails (R1-15). Break 2: make the
  tree fold its own coverage over the primary callers' uncertainty (the cut's fold) → the
  equality fails on a routine whose only four-kind edge is in dependency code.
- **T9 (`--diff` roots, a USE pin)**: digest resolves its roots from the table it passes into
  the plan, and on every cli-b diff fixture the roots equal today's. A second assertion: the
  table is unfiltered (it holds the dependency fixture's dependency rows). Break: pass a table
  with one primary routine's row removed → that root disappears → fails (R1-17).

### 5.9 Tests deleted or rewritten with the code they pin (I2's exception)

Deleted: `session.rs:676-680` (`a det cell never carries RAW_INHERITED_FACTS`), the
`should_panic(expected = "RAW_INHERITED_FACTS")` test (`capability_query.rs:295`), the timings
mode's smoke test (`tests/cli/aldump_smoke.rs:100`). Rewritten: the `ConeOutput` parity tests
on `ConeScope` (`cone_derived.rs:1195-1780`); the uses of `build_r3a3_source_only_base` in
`detector_context.rs:2094`, `detector_context_step5_tests.rs:283` (its claim becomes T8's),
`detector_context_11b_tests.rs:1968`, `capability_cone.rs:6333`, `:6371`, on `compose_tree`;
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
- **Part 3 (reported, no bound; Decision 58).** 3 rounds: cross-app `policy check` (wall and
  peak, expected to DROP), cross-app default `analyze`.

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
raw `Vec`s and facts, `non_primary_members`, the composition ms (**K**), the tree derive ms
(**T**), the effects ms (**E′**), scoped pd states/links, `PD_EDGE_UNMAPPED`, anchorless rows
with links, ordering kept roots, `dependency_calls` count and bytes, single-app body-less rows.
Each runs 3 rounds through `maxrss.py` in the quiet slot. **S_P** = the parent's
`ordering.compose_snapshot` + `ordering.digest_query` for `o` (cut base + cut derive + cut
effects), and `snapshot.r3a3_base` + `snapshot.derive` for `d`, both from 5 parent rounds in
the same slot. The ledger records K, T, E′ and S_P (medians and bands) before Task 2.

- **S1 (wall).** `d`: `K_d − det cone_P + T_d + E′_d − S_P(d) ≤ Bd`. `o`: `K_o + T_o + E′_o −
  S_P(o) ≤ 0.5 × Po` (the cap of 6A-10).
- **S2 (memory).** The probe's process peak per configuration (KiB, `maxrss.py`), against the
  parent's process peak of the same command: `o` and `d` ≤ 1.03 × parent; policy ≤ parent
  (R1-04, R1-24). The probe stops at the effects, so its peak is a lower bound for the
  command's; B2/B4 bound the whole command.
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
  the row deletes. B4 stays as worded.
- **6A-11 `identities` is a command-local of digest until step 12**, built before `prepare`
  and moved into the tree; with one job per view a cell has no second reader.

## 10. Fallbacks (every "if the measurement shows X")

Every fallback that changes code re-runs every identity gate it could affect (T3, T4, the
golden targets, §8 Part 1) and the cost and scope checks (S1, S2, T8) before the claim
continues; each is recorded in the ledger.

| # | If | Then |
|---|---|---|
| F1 | — | withdrawn in round 1: raw is all alive when the composition returns, so releasing it after use cannot lower the peak (R1-01) |
| F2 | S1 or S2 fails on `o` (first rung) | gate the raw walk by the three ordering bits (Decision 38, `map_op`'s vocabulary): the composition turns `derived` on for the ordering bits only and materializes raw only for roots with an ingredient; Task 1's probe prices the extra fold; T3's ordering arm proves the kept-root set unchanged |
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
