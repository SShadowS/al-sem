# #160 epic: one analysis per revision, commands as projections, cross-app fingerprint/prove/diff

Status: epic design, pre-implementation. Restructured after review round 6 (2026-10-18):
rounds 1 to 5 kept finding real problems in the DETAILED design of late delivery steps,
whose code will look different once the early steps land. This document now holds the
contract: goal, architecture, decisions, invariants, the delivery order with per-step
acceptance, and the findings each step's child spec must resolve. Implementation detail
lives in child specs, one per delivery slice:

- `2026-10-18-issue-160-steps-0-2-design.md` — steps 0, 1, 1b, 2 (written).
- steps 3–4, 7a–7b, 10–11, 5–6, 8–9, 12, 13 — each written when its predecessor merges,
  against the code as it then is.

Base: master `1e54624a`, worktree `U:/Git/al-sem-issue-160-a2` (head `5eca5bb5`: no
`src/`, `tests/` or `crates/` change since `1e54624a`; the first attempt's worktree,
`…-a1`, is gone). Every code claim cites `file:line` in that tree. Sources: `.agent/runs-probe-160.md` (probe Pn), the cone-timing
census (`scratchpad/cone-timing/`), Astra's review (`scratchpad/astra-160.md`), verdicts
`disc-digest` (bug #161), `disc-deprole`, `disc-mid`, critiques rounds 1–6
(`scratchpad/critiques-160-r{1,3,5}.md`).

Words: **revision** = one frozen input set and everything computed from it. **View** = one
analysis universe over a revision (primary-only or cross-app). **Cone** = which facts (table
writes, commits, UI, HTTP) a routine reaches through callees. **Substrate** = one owned
artifact the detectors read. **SCC** = a group of routines that call each other
recursively. **Fold** = turning call-site uncertainty kinds into `partial` coverage.
**Cell** = one owned artifact of a `View`, filled once, released after its last reader.

## 1. Goal

1. Each artifact of a revision is computed **once per process**, whatever mix of commands
   reads it. No second detector context, cone, v2 solve or combined graph for data a
   command already has.
2. **One execution relation** with named transfer rules replaces the graphs each command
   builds for itself. Fixes bug #161 by construction.
3. **One coverage-fold policy** for every consumer, both universes.
4. `fingerprint`, `prove`, `diff` answer behavioural questions **cross-app by default**;
   ABI and schema inventory stay primary-app.
5. Commands are **compute, project, serialize, write**. Only the binary writes.
6. The cone separates **membership** (which facts) from **witnesses** (which path).
   Witnesses only for subjects whose raw output is projected.
7. The span-join adapter between program engine and model is deleted; one canonical
   identity (`RoutineIx`, `SiteIx`) on both sides.

## 2. Non-goals

- No persistent cache, no revision key; the frozen `InputSet` value is the identity.
- `modelInstanceId` never becomes a cache key (it hashes `guid@version` and file paths,
  `src/engine/gate/model_instance_id.rs:75-95`).
- No daemon. The incremental host (step 13) is scoped only to its acceptance test.
- The L4 store's dependency-leaf contract stays (`capability_cone.rs:3534-3548`).
- No salsa adoption (pinned `=0.28.5`, `Cargo.toml:125`; step 13 adopts or removes).
- `al-call-hierarchy --project` (LIGHT, ~1 s, no model) stays outside batch.
- No byte compatibility with schema-3 snapshots; diff refuses a mixed pair.

## 3. Background and measurements

CDO (`U:/Git/DO-cdo-baseline/Cloud` at `bc3ccb18`), `release-fast`, loaded dev machine,
noise about ±8 % (probe P5).

| Command | Wall s (run 1 / 2) | Peak WS MB | Output |
|---|---:|---:|---:|
| analyze (sarif) | 43.6 / 37.5 | 2846 / 2831 | 9.7 MB |
| digest (`--diff`, 48 roots) | 66.8 / 63.7 | 2838 / 2843 | 227 KB |
| fingerprint (json) | 7.5 / 7.0 | 1385 / 1384 | 129 MB |

Analyze's 36.1 s (traced): snapshot + parse ~2.0 s; resolve ~3.0 s; model assembly ~8.7 s
(`attach_program_calls` 6.0 s, of which the span-join adapter `b3.adapter` 4.8 s); detector
context 21.6 s (`context.capability_cones` 20.9 s, 60 %); detectors ~0.2 s; gate ~0.4 s.

Duplicate work today (probe P5): digest builds the 21 s context **twice**
(`src/engine/l5/digest_cli.rs:1655-1660` for diagnostics; `:534-560` for the tx map, which
reads only `transaction_spans`). A cross-app digest builds the combined graph 5 times and
runs the v2 solve 4 times and the cone 3 times (counted at the top-level call sites, child
spec §4.1). prove, events and policy each build two contexts (`src/engine/l5/prove.rs:843`,
`:902`; `src/engine/gate/events.rs:406`, `:422`, `:567`; `policy/pipeline.rs:216`, `:261`).
Every diagnostics run parses the workspace a second time
(`src/engine/gate/workspace_diagnostics.rs:119-129`). prove computes digest effects for
every routine though it answers one root (`prove.rs:961`).

Inside the 22.6–22.9 s cone (`scratchpad/cone-timing/census1.sarif.err`): recursive BFS
10.0–10.5 s over 714 calls (**the timer also covers `emit_picks`'s per-pick fold**,
`capability_cone.rs:2551-2580`, finding G-3); singleton walk 5.7 s over 30,942 calls
(scan 3.1 s, fold 1.8 s); compose remainder 4.0–4.8 s untimed, containing
`pd_solve::solve` (`:2447`); SCC fact-cone composition 1.6–1.7 s over 28,191 SCCs; graph,
Tarjan, dedup, coverage, emit < 0.25 s. Populations: 31,658 routines; largest SCC 479;
`cone_entries` 15.07 M (**includes own producer entries**, `:1856-1857`); producer solve
3.53 M states, 4.94 M links, 3.53 M entries, 525 MiB. After the span 98 MB stays
(`ConeDerivedStore` 62 MB). The process peak (2,847 MB) is reached in
`crossapp.attach_program_calls` (`cone-timing/spans.txt:6`), before the drop at
`program_calls.rs:669-676` (−1,512 MB); every later span keeps that number.

What the probe falsified in the original ask (P6): sharing the model alone saves 13 %,
inside noise; sharing the context and default run saves 54 %; removing digest's duplicate
context alone saves ~21 s; byte identity between separate runs holds only with
`--deterministic` or SARIF.

## 4. Root cause

**How long an analysis result lives is decided by the command, not by what was analysed.**
`run_detectors*` builds and drops its `DetectorContext` inside the call
(`src/engine/l5/registry.rs:395-416`, `:455-529`); `attach_program_calls_with` drops
`ProgramContext` and `ProgramReport` once the model exists (`program_calls.rs:669-676`);
`compose_snapshot` builds its own graph and cone (`src/engine/l5/snapshot.rs:1150-1156`;
`capability_cone.rs:4015-4101`). Because each consumer chose its own graph, the graphs
drifted, and that drift **is** #161: the snapshot graph lacks the injected dependency edges
the context appends (`capability_cone.rs:4019-4021` vs `detector_context.rs:940-945`), and
the snapshot folds four uncertainty kinds into coverage (`:4047-4064`) while the context,
the cross-app base and r3a5 do not (`detector_context.rs:983-988`; `:3550-3557`).

The two issues have two causes. Lifetime explains #160's duplicate work. #161 comes from
each consumer building its own relation and its own fold, so a longer-lived context
alone would not fix it. The architecture fixes #161 through one relation with named
transfer rules (§5.5, `Transfer::Cone`) and one fold policy (§6).

The fix is structural: one relation with named transfer rules, owned by the view, and
artifacts that live as long as the view, not as long as the call.

## 5. Architecture

### 5.1 Frozen inputs (`src/program/inputs.rs`, step 1)

`InputSet` holds, read ONCE: the workspace path as spelled; `app.json` (raw text and its
parse, with every field the five readers use today); every `app.json` path found (the
multi-app check); `roots.config.json` bytes; `al-sem.coverage.yaml` bytes; the dependency
closure (closure-v2 discovery); `derive_inputs()`'s rows; every workspace `.al` file from
the `NestedApps::Walk` list with `rel_posix`, `abs_path`, `nested_app` and
`text: Result<Arc<str>, String>` (an unreadable file keeps its slot; the program build fails
on it as today, the diagnostics warn from the slot with the same text); `driver_version`;
`model_instance_id: Option<String>`. Every disk read after the semantic build takes
`&InputSet`. The second workspace parse becomes a per-file `has_object` bit from the first
parse. **Size: the `.al` text is 5.09 MB on CDO (582 files) and grows with the workspace;
it lives for the process (step 1b census, finding G-6).**

### 5.2 Retained resolved program (`src/program/resolved.rs`, step 1b)

`ResolvedProgram { meta: ProgramMeta, body: Option<ProgramBody> }`. `ProgramMeta` (inputs,
app set, `primary_app_ref`, `has_object`, `recovered_files`, `FreshCoverage`, both
histograms, ABI integrity, loaded apps) lives as long as the view. (Round 10: the app
set, ABI integrity and the build profile join `ProgramMeta` at the first step whose
reader needs them after the body is released; steps 0–2 have none, so the step-1b struct
holds only the fields they read, child spec §4.3.1.) `ProgramBody` (the
`ProgramContext` with its graph, parsed units and dependency bodies; the report's edges in
**two sets kept apart**; `site_facts`; `parenless_calls`; obligation coverage) is released
at today's drop point: right after the model, before any graph (§5.9 rule 3 item 2).
The typed `FileTable`/`SiteTable`/`RoutineTable` come with steps 10/11.
(Amended 2026-10-19, step 1b code review r1: `body` is a plain `ProgramBody`, not an
`Option`; the `graph` job borrows the program and the model builder consumes it and
returns the `ProgramMeta`, which the holder keeps from then on. A view therefore holds
the program until the model is built and the meta after, never a program with no body.
The build takes a `ProgramScope` (`Workspace` | `CrossApp`) and `ProgramMeta` records it.
Child spec §4.3.1.)

**The two edge sets.** `edges` is today's `report.edges` before `build_cross_app_program`
extends it (`src/program/resolve/full.rs:2176`): every workspace routine's edges PLUS
event-flow edges from publishers in every app (`full.rs:259-260`; the publisher loop at
`resolver.rs:3293`). `dependency_body_edges` is `resolve_dependency_bodies().edges`. The
histogram describes `edges` exactly (`:2170` vs `:2176`). `graph` projects
`edges ++ dependency_body_edges`, today's merged order, and **the order is kept because
the projection merge is order-sensitive on a `conf` tie (the first edge's `via` wins,
`impact_graph/project.rs:277-291`)** — finding R6-5 corrected the earlier "no caller sits
in both sets" justification. `graph` reads `ResolvedProgram` with no demand filter and no
model; the LSP keeps its LIGHT path until step 13.

### 5.3 One canonical identity (steps 10, 11)

`RoutineIx(u32)` over physical occurrences (program rows `src/program/physical.rs:44-56`,
model rows, dependency source rows); `SiteIx(u32)` keyed `(FileIx, ExprId)` (both walks
know the expression: `extract.rs:164`, `ir_walk.rs:75-80`). Deletes joins J1 (span,
first-edge-wins, `program_calls.rs:420-535`), J2 (`:933-943`), J4 (source-text op name),
J5 (`events.rs:290-312`) and the `texts` map; keeps J3 (ABI zip, `workspace.rs:1491-1531`).
`RoutineNodeId` stays the deduplicated program identity. Compatibility string ids are a
projection computed once per table. **Collided string ids** (today three merge rules:
edge union `combined_graph.rs:384-393`, last-wins facts and coverage
`detector_context.rs:986-987`, first-wins summary `:1108-1123`) become per-occurrence in
the engine with ONE deterministic, order-independent merge in the string-id projection
(Decision 6).

### 5.4 Model, one per universe

`Universe { PrimaryOnly, CrossApp }`. A single-app model is not a slice of a cross-app one
(`finish_resolved` re-runs `resolve()`, `workspace.rs:1751-1757`;
`prove_event_param_temps` rewrites rows, `program_calls.rs:688`). Rows gain `ix`; call
sites and ops gain `site`.

### 5.5 One execution relation (`src/engine/l4/exec_relation.rs`, step 4)

```rust
pub struct ExecRelation {
    pub typed: Vec<TypedEdge>,         // Cone payload; TODAY'S CONCATENATION ORDER (Decision 43)
    pub combined: Vec<CombinedEdge>,   // Primary payload: today's edges_by_from rows (Decision 39)
    pub uncertainty: Vec<UncertaintyEdge>,  // every caller (today the dependency half is dropped)
    pub events: Arc<EventGraph>, pub upgraded_bindings, pub external_targets,
    pub role: Vec<Role>,               // per RoutineIx: Primary | Dependency { app }
    pub dependency_calls: Vec<CallEdge>,   // dependency-caller half, with per-site `resolution`
    pub from_order: Vec<RoutineIx>,    // first appearance per `from` in emission order (d17)
    adj: [Adjacency; 2], scc: [SccResult; 2],   // EAGER in `new`; no lazy cell
}
pub enum Transfer {
    Cone,     // every typed edge, every caller: membership, coverage, witnesses, pd_solve
    Primary,  // Primary callers' call edges PLUS event dispatch from ANY caller
              //   (today's edges_by_from): reverse graph, spans, temps, L4 solve
}
```

Built once per view from the model's uncut `ResolvedCalls` split by caller role, then
`combined` from the Primary half plus the event graph as `build_combined_graph` does
(`combined_graph.rs:260-378`) and `typed` as two builds concatenated (Primary half with
events, then the dependency half with none, `capability_cone.rs:3496-3515`). Replaces the
cut (`:3459-3469`), the injection (`:3496-3515`), the context's rebuild
(`detector_context.rs:935-945`), the snapshot's graph (`:4019-4021`) and the graph parts of
`R3a5CrossAppBase`. **Field contract** (step 4 reproduces it byte-for-byte):

| `DetectorContext` field | `Transfer` |
|---|---|
| `graph.typed_edges` (d43; the cone) | `Cone` |
| `graph.edges_by_from` (d1 d2 d3 d7 d13 d14 d16 d17 d34 d45 d46 d48 d54; `event_flow.rs:731`); d2 reads `resolution`, `subscriber_app_id` | `Primary` as `rel.combined` by `from` |
| `graph.edges_from_order` (d17, first-wins sample) | `Primary` as `rel.from_order` |
| `reverse_call_graph`, `closed_world_temp_params`, `transaction_spans` | `Primary` |
| `uncertainty_edges_by_from`, `uncertainties_by_node`, `resolved_call_edge_by_callsite` | `Primary` (§6.3 widens the FOLD only) |
| `db_effect_bundle`, v2 solve, leaves and fixed roles | `Primary` |
| `entry_points`, `reachable_roots` | none (routines + roles) |
| `event_flow_indexes`, `cross_extension_subscribers` | `rel.events` |

`Model.calls` in a cross-app view holds the Primary half (today's cut). In `PrimaryOnly`
both rules give today's adjacency. The snapshot asks for `Transfer::Cone`: the structural
fix for #161.

### 5.6 Capability analysis: membership separate from witnesses (steps 7a, 7b, 6)

`CapabilityAnalysis { pd: Option<PdArena>, coverage: Vec<CoverageRecord>, derived:
ConeDerivedStore }` is what is retained. The interner, hash-consed sets and fold memo are
build-local (Decision 32). Membership(r) = `static_members[scc(r)] ∪ own(r) ∪ pd_keys(r)`;
the derived row is a monotone union over key fields (`cone_derived.rs:43-69`), so one
inherited set per Cone-SCC replaces the per-member BFS, and a per-`SetId` fold memo plus a
new `ConeDerivedBuilder::fold_row` replace the per-fact fold for closed (not frame-bound)
entries. Open entries (frame-bound, guarded) still go through `substitute_entry`. Three
ordering bits per row, keyed on `op` alone with `map_op`'s vocabulary (Decision 38).

**The `pd` arena, one copy, view identity** (Decision 42, re-scoped by G-1/G-4): `pd_solve`
emits the link graph (`states`, `links: (StateIx, EdgeIx)`, `by_routine`) with `RoutineIx`,
`EdgeIx` into `rel.typed` and `DirectFactIx` into the view's direct-fact table; about 24 B
per state plus 8–16 B per link (≈150 MB on CDO's full graph against 525 MiB raw today).
**The witness pass also needs the solve's resolved own entries** (`own_entries(m)`,
`capability_cone.rs:2448`, read at `:2559` and `:2571`; `ResolvedEntry<'g>` borrows the
graph, `pd_solve.rs:89`): either `caps` retains them in view identity (3.53 M entries,
budgeted) or the pass re-solves. **The step 6 child spec decides, with the step 0
`pd_solve` counter and timer as its instrument** (carried finding G-1). The arena lands
with step 6, not step 11, because `EdgeIx` and `DirectFactIx` first exist there (G-4).

### 5.7 The witness engine, scoped (step 6)

`compose_cone_over_graph(.., RawOnly)` stays the witness engine. Its `nodes` list today
plays two roles (graph universe, `:2773`; walk set, `:2801-2803`, `:2539-2541`); they
split: the universe is every routine of the view, always; `WitnessScope.subjects` restricts
ONLY the per-routine inherited walk. Everything else (direct facts, coverage, typed edges,
evidence, ledger, declarations, roots) is per routine of the view, in one shared
`ViewTree`; a job's `SnapshotTree` is a subject set plus `Arc` clones of the pass's facts
(one copy per subject across every job) plus an **effects cell** (per-root digest effects,
the only reader of `caps.pd`). Subjects per consumer: fingerprint/diff = every primary
routine; digest = changed roots; prove = `[root]`; ordering facts = reportable roots whose
membership carries an ordering ingredient; policy = every primary routine (its raw facts
come from the pass; `RAW_INHERITED_FACTS` is deleted, Decision 16); `--inventory-only` =
none. One pass per view at the union of subjects, before `det`. **How much of the
`ViewTree` stays alive through the detector runs, per job, is open** (carried G-2): the
step 6 child spec either shrinks a digest-only plan's retained tree to what its compute
reads (coverage, identities; edges and evidence only when prove is planned) or runs the
digest/prove compute before the runs, and budgets `ViewTree` bytes from the step 0 census.
`ViewTree.typed_edges` keeps one row per `rel.typed` edge at the same index; serialization
re-applies today's skip of anchorless rows (`snapshot.rs:1395-1501`), and the rule for a
`pd` link into such an edge is stated there (carried R6-4).

### 5.8 L4 analysis, one builder (step 2)

`L4Analysis { bundle, summarize diagnostics, warnings, scc (an `Arc`, shared with the
cross-app base) }` (the field index is a solver input with no reader after the solve, so
it is a local of the builder and is not retained; child spec §4.4.1, Round 13) over `Transfer::Primary` (at step 2: over the context's
combined graph; step 4 swaps in `rel`), plus the transient, UNHARVESTED core-summary
map (`CoreSummaries`) that the `det` builder harvests into uncertainties and parameter
roles. The `det` builder calls the `l4` builder itself, after its own cone, so the map is
a local of that call (child spec §4.4.1, Round 11). The harvest is up to ~730 MiB on 8020, only `det`
reads it, and it also reads the context's `uncertainty_edges_by_from`, so it runs in
the `det` builder and never on a plan with no `det` fill (query). It must not outlive
`det` in digest and prove (child spec §4.4.1, reviews B8 and a2-r1 2-8; amended
2026-10-18). Replaces the context's inline solve
(`detector_context.rs:1387-1396`), `cross_app_db_effect_bundle`
(`effect_query_cli.rs:207-220`) and `build_db_effect_bundle` (`:226-262`).
`compute_uncertainty_coverage_reasons` (`capability_cone.rs:3185-3247`) is deleted by §6.3.

### 5.9 `View`, cells, the driver (`src/engine/session.rs`, step 2 onward)

```rust
pub struct View {
    pub program: Arc<ResolvedProgram>, pub universe: Universe, pub model: Model,
                                           // `program` joins at the first step that reads it
                                           //   through the view; step 2 has no such reader and
                                           //   holds the target alone (child spec §4.4.3, I3)
    pub rel: ExecRelation,                 // step 4; step 2 holds today's cross-app base here
    caps: Option<CapabilityAnalysis>, l4: Option<L4Analysis>,
    det: Option<DetectorArtifacts>,        // owned half of today's context, ONE per view,
                                           //   built at the union of the plan's masks, never raw facts
    ordering: Option<OrderingFacts>, identities: Option<IdentityTable>,
    trees: BTreeMap<JobId, SnapshotTree>, runs: BTreeMap<RunKey, Arc<RunOutput>>,
}
impl View {
    pub fn prepare(&mut self, plan: &Plan);   pub fn release(&mut self, cell: Cell);
    pub fn detector_context(&self) -> DetectorContext<'_>;   // cheap borrowed view
    pub fn run(&self, key: &RunKey) -> &RunOutput;  pub fn tree(&self, job: JobId) -> &SnapshotTree;
}
```

**Rules every cell follows.**

1. **Filled serially from built inputs, never from a worker.** A cell builder takes `&T`
   inputs `prepare` already built, never `&View`. `prepare` fills in dependency order
   before any parallel loop. The types forbid a fill from a detector (a detector takes
   `&DetectorContext`, every fill is on `&mut View`); one thread-local backs the rule in
   release builds: `IN_DETECTOR_LOOP` (`registry.rs:540-566`), asserted clear at
   `prepare`'s entry. (`IN_CELL_BUILD` withdrawn, child spec review B5: set by `prepare`
   around a fill and asserted clear by the fill, it contradicted itself, and a rayon
   worker inside a fill would not see it.) A read asserts only "filled". A cell that
   recurses over source is built under `big_stack_pool().install` (the ordering fill
   today, `registry.rs:605-621`). No `OnceLock`, `Mutex` or lazy fill anywhere in `View`
   (Decision 3).
2. **What a cell holds never changes output.** A run's summarize diagnostics come from the
   mask that run demanded (`registry.rs:40-44`); `RunKey` = (detector set, demanded mask).
3. **Last use drops; the heavy passes run once; the order is fixed, the same for every
   plan:** (1) program build, then the `graph` job; (2) `model`, then `ProgramBody`
   released, then `rel`; (3) `caps` with one `pd_solve`; (4) `identities`; (5) the one
   witness pass at the union of subjects, each job's tree projected at once; (6) the
   effects cells, then `pd` released; (7) `det` and `l4`, then the runs. While `det`
   builds its own cone, the `det` builder runs that cone FIRST, then the `l4` solve, then
   the harvest. That is today's order, so the solve's output (bundle and core map) is
   never alive beside the cone's working set (Round 11; child a2-r4 1). A plan with no
   `det` fills `l4` alone; (8) small projections off
   `det` (tx maps, fanout, and from step 6 the policy rule engine), then `det` released.
   Before step 6 the policy rule engine is NOT an item-8 projection: it reads raw
   inherited facts, which no `det` may carry (I5, Decision 50), so it runs after `det`
   is released, off policy's command-local RAW build (child spec §4.4.4; round 9);
   (9) every job's
   `compute` (a `Doc` borrows nothing), then `caps`, `model`, `rel`, `l4`, `identities`
   released; (10) serialization, one job at a time. Before step 11 every cell keys routines
   by the model's string id (Decision 25). Before step 6, items 5–6 do not exist: digest's
   and prove's entries come from today's composed snapshot, built at item 9 AFTER `det`
   is released (child spec §4.4; G-2's step-2 half: the snapshot never meets `det`).
   `l4` (bundle, SCC) is still alive during that composition, because `DbTempStates`
   borrows the bundle after it (`digest_cli.rs:744-761`); today digest builds its bundle
   after composing. That is the one new overlap at step 2, in both universes, for
   digest and prove: the `l4` census bucket sizes it and the step-2 peak bound covers
   digest and prove (child spec §4.4.4, Round 13). It is not claimed flat.
4. **Census counters are process-cumulative, never reset.** `cones_census::report()`
   prints running totals; a numbers file quotes the process total, so two steps' lines
   measure the same thing (child spec §4.1.7, review D6; a per-fill reset was the
   earlier text).
5. **The thread budget is `RAYON_NUM_THREADS`; no flag, no code** (Decision 36).

### 5.10 Commands as projections, the transport (steps 2, 12)

`compute(&View, &Req) -> Doc`, `project`, `serialize(&Doc, &Presentation) -> JobOutput
{ stdout, stderr, stderr_first, files, exit }`. Only `src/bin/alsem.rs` writes, after
compute; `--update-baseline` (`run.rs:584-591`) and the shard directory
(`fingerprint_cli.rs:586-596`) move out of compute. **One writer, one write-failure rule:**
a failed write to stdout, stderr or a side-effect file prints
`al-sem: write error: <stream|path>: <OS message>` and exits with the command's own
failure code: 2 (`ANALYSIS_FAILURE`) for analyze, 1 for every other command (today
analyze's `println!` under `panic = "abort"`, `Cargo.toml:268`, aborts; the eleven other
stdout/file sites in `alsem.rs` exit 1 with four different texts; Decision 49). A
uniform 2 is not possible: digest exits 2 for "no changed roots" (`digest_cli.rs:1652`)
and prove for "routine not resolved" (`prove.rs:950`). Late
validation moves before the build for fingerprint `--shard`/`--out` and policy load, each a
stated precedence change (Decision 7); `diff --renames` keeps its order. analyze's
cross-app failure path stops re-running the single-app build; the classification rows are
in the child spec (§4.4, Decision 29 as corrected by R6-3). The snapshot has two phases:
`identities` (unfiltered, every row) before any witness pass; roots resolve from it.

| Command | On `View` |
|---|---|
| analyze | `run(selected)`; diagnostics from that run; project, serialize |
| digest | `run(default)`; tx map from `det.spans`; `l4.bundle`; roots from `identities`; S4 entries over the changed roots (effects cell from step 6); `run_digest_query_full_from_entries` |
| prove | cross-app from step 9; `run(default)`; `det.spans`; `l4.bundle`; `Some(root)`; `unresolved_cone` over the tree |
| fingerprint | cross-app from step 9; `run(default)`; primary-subject tree; `--inventory-only` reads model, roles, `identities`, `caps.coverage`, no pass; the query path filters by role (Decision 28) |
| diff | cross-app from step 9; view A, tree, drop; then B |
| query | `l4` only; mask `CORE_SUMMARIES` (Decision 40) |
| events | `run(default)` + `det.event_flow_indexes` |
| policy check | `run(default)`; rule engine as an item 8 projection off `det`; `facts: inherited \| any` from `tree(policy)` |
| graph | `ResolvedProgram` only |

### 5.11 Determinism

Compute output holds no presentation data (`generatedAt` moves to `serialize`). Order
never comes from which job filled a cell: `RoutineIx` order is today's row order; `rel.typed`
keeps today's concatenation; `rel.from_order` is recorded from emission order; dependency
bodies resolve in `snap.apps` order; interning order never reaches output; Rayon only with
indexed `collect`. Library stderr belongs to the artifact: the non-convergence warning
(`summary_runner.rs:1811-1814`) is stored on `l4` and emitted once per job whose mask has
`CORE_SUMMARIES` (Decision 30). Batch: `--format auto` = Json; one `--diff -` reader; files
written after compute in job order. Unchanged: stream order, trailing newline, the
exit-code table (`exit_code.rs:3-9`, plus the write-failure row), diagnostic source order
(`run.rs:1085-1117`), CBOR key order.

### 5.12 Memory model

Every peak bound is **two numbers, both relative**: the process peak and the
**post-attach phase peak** (max RSS from the end of the attach drop, `b3.ctx_drop`, to
exit), each ≤ the parent-commit interleaved median + the peak noise band (3 %, about
85 MB on CDO). 2.85 GB is a reported ceiling, never a bound (Decisions 22, 33). The process
peak is set in attach (2,847 MB) about 1.3 GB above the live set that follows, so it alone
cannot fail a late retention. Steps with a peak bound: 1b, 2, 6, 9, 10, 12; steps 2, 6, 9
also take a cell byte census at the first detector run. Fallback ladder for a miss: (1) a
census at the serializer shows no `det`, `caps`, `model`, `rel`, `l4` bytes; (2) name the
miss (runs vs serialization spike); (3) stream the serializer (Decision 51). Four workers
(step 12): summed peak ≤ physical RAM − 4 GB; throughput ≥ 0.8 × 4 × one process at
CPUs/4 and ≥ one process at full threads (Decision 26).

## 6. Cross-app semantics

- **Roles, one rule** (step 8): `Role` from `app_guid` against `Model::primary_app`, for
  routines and objects (object-anchored findings, `registry.rs:476-500`); replaces the
  snapshot stub (`snapshot.rs:2219-2223`), the registry's assignment (`:481-500`) and the
  base's id set (`capability_cone.rs:3449-3454`). The `dep:` location filter
  (`registry.rs:501-515`) stays beside it.
- **One cone graph** (#161 part 1, step 6): the snapshot reads `Transfer::Cone`; its
  call-site ledger reads `model.calls.edges` then `rel.dependency_calls` (Decision 18). A
  resolved edge into a body-less dependency row is ledgered `unfetched-dependency` AND
  emits a gap (Decision 8). Dependency routines stay fixed leaves in the solve.
- **One coverage-fold policy** (#161 part 2, step 5): a routine's direct coverage is its own
  body plus the four call-site-local uncertainty kinds (`ambiguous-overload`,
  `member-not-found`, `external-target`, `interface-open-world`) on its own edges in
  `rel.uncertainty`; only the cone carries coverage up. No solve needed
  (`is_callsite_local_kind`, `db_effect_solver.rs:835-840`); step 3's oracle checks the
  SCC-member filter (`own_by_member`, `:983+`) first. Readers that move: transaction spans
  (`transaction_spans.rs:228-240` → `TX_UNKNOWN`), `events fanout`, d35, d45, the policy
  coverage gate (`policy_engine.rs:236-238`; fail-closed replaces the rule's finding,
  `:241-247`). Goldens move in both universes.
- **Gaps** (step 8): `is_dependency_role` = `role != Primary`; a gap is emitted only for a
  dependency routine reachable from a primary subject under `Transfer::Cone`
  (`demand.rs:29-84` can hold more). `parse-incomplete` gets the same filter.
- **Snapshot schema 4** (step 8): `analysisUniverse`; `apps[]` primary first by role;
  preflight refuses schema 3 and a universe mismatch. **Every top-level key has a class:**
  Input (from `InputSet`: `workspaceFingerprint`, `inputs`, `inputsMetadata`), Inventory
  (primary rows: `apps`, `contractFacts`, `schemaFacts`, `permissionFacts`,
  `rootClassifications`, `eventDeclarations`), Behaviour (primary subjects:
  `capabilityFacts*`, `coverage`, `callsiteResolutions`, `typedEdges`,
  `routineOrderFrames`), Gap (`analysisGaps`, reachability-filtered, Decision 46),
  Evidence (`operationIndex`, `callsiteIndex`, the dependency rows of `identities` that an
  emitted row names). `--single-app` on fingerprint, prove, diff. No-dependency contract:
  cross-app output equals single-app except `analysisUniverse`.
- **#161 acceptance** (disc-digest): on the `mk.py` fixture, cross-app
  `digest --routine CallChain` reports DB_MODIFY on dependency Table 18 and COMMIT via
  `Dep Lib::Outer → Inner`, matching `aldump --r3a5-cross-app-summary`; per-root coverage
  equals the detector context's.

## 7. Invariants every step must preserve

Each child spec names which of these its step touches and how it proves them.

- **I1 Build counts.** Per view: at most one detector context, one `l4` solve, one cone, one
  combined graph (after step 4: one `ExecRelation`), one `pd_solve`, one witness pass.
  Instrument: the step 0 build counters, read from the trace per command and universe
  (child spec §4.1); the table of expected counts is restated by every step that moves it.
- **I2 Byte-neutral means byte-neutral.** A step labelled byte-neutral moves none of the 8
  golden targets, no `--lib` test outcome, and the CDO `aldump` histogram SHA; stderr
  changes are stated, never hidden under "byte-identical". One exception: a `--lib` test
  that pins the very code a step deletes or rewrites (a lazy fill, a pre-loop fill, a
  `debug_assert!` that becomes an `expect`) is deleted or rewritten in the same commit as
  that code. The child spec names each such test and its replacement, and lists it in
  its §7 (Round 11; child a2-r4 M4).
- **I3 Lifetimes are the driver's.** No `OnceLock`, `Mutex` or lazy fill in `View`; cells
  take `&T`; `prepare`/`release` through `&mut`; the fixed order of §5.9 rule 3.
- **I4 Output never depends on fill state** (`RunKey` carries the mask) **or on which jobs
  share a view** (fixed order, one pass at the union of subjects).
- **I5 No dependency subject ever gets a raw inherited `Vec`; no `det` ever carries one.**
  Census count must be 0, policy included.
- **I6 Every bound is relative and can fail.** Wall: a saving against the parent-commit
  interleaved median of the SAME sub-phase timer, minus that timer's own noise band
  (band = max − min of the parent runs, ≥ 3 runs). A whole-command wall's band is
  defined the same way (max − min of the parent's wall runs); the earlier "8 % of the
  command median" is withdrawn and applies to no bound. When the step DELETES the
  sub-phase, the head has no such timer: the saving is then read off the whole-command
  wall (parent median − head median), and the bound subtracts the wall's own band as well
  as the timer's (step 2's digest bound). A sub-phase bound is written only when
  the step 0 median of that timer ≥ 2 × its band; otherwise the step is re-labelled
  non-speed before its spec is written (finding G-5). Peak: both numbers of §5.12.
- **I7 Order is recorded, not assumed.** `rel.typed` concatenation, `from_order`, row
  order, dependency-body order, Rayon indexed collect.
- **I8 Serialized inventory is primary-only; behaviour is primary subjects; in-memory
  tables are unfiltered.**
- **I9 A test pins the use and proves discrimination** (CLAUDE.md). Every new test in a
  child spec lists its break, the failing assertion, and the revert.
- **I10 CDO ratchets unchanged** (`scripts/cdo-gate`) on every non-docs step; steps that
  move findings run `cdo-measure` and triage.
- **I11 The `.alpackages`/`.dependencies` law:** nothing special-cases a folder name.

## 8. Decisions

Architecture forks a critic exposed; the choice and reason are recorded so no later reader
reopens them without new evidence. Numbers are stable across rounds.

1. Two transfer rules, `Cone` and `Primary`, not three; `DbEffect`/`Traversal` read the same adjacency (`reverse_call_graph.rs:36-46`).
2. `WitnessScope` is a subject set, not a root closure, and lands in step 6 (`capability_cone.rs:2539-2541`; the 10.9 GB shape, `registry.rs:63-70`).
3. The driver owns the `View`; no `OnceLock`, `Mutex`, lazy fill; `prepare`/`release` through `&mut`.
4. No retained workspace AST; a per-file `has_object` bit (`workspace_diagnostics.rs:119-129`).
5. Identity work (10, 11) before semantic work (5, 6); step 4b dropped. Step 10 is bounded (37, 45).
6. Collided string ids: per-occurrence engine, one deterministic projection merge; hand-stated fixture before the switch.
7. Late-validation moves are stated precedence changes; diff keeps its order (`cli_diff_strict.rs:279-302`).
8. Symbol-only callee: ledger `unfetched-dependency` AND a gap (`program_calls.rs:1107-1111`, `:1341-1345`).
9. `--inventory-only` keeps `coverage` (a membership artifact).
10. Speed is a bound, not a report.
11. Thread budget process-wide — **superseded by 36**.
12. One `det` per view at the union of masks; no mask carries raw facts; parity test widened to superset masks.
13. Every snapshot key has a class.
14. `WitnessScope` scopes the inherited walk only; every other per-routine artifact is per routine of the view; "exact by construction" withdrawn, the scoped-equals-unscoped oracle replaces it.
15. The witness pass runs before `det`, in every plan; the retained-tree cost is budgeted (re-opened by G-2 for digest: carried to step 6).
16. `RAW_INHERITED_FACTS` deleted; policy reads the witness pass.
17. In-memory identity table unfiltered; serialized key = inventory plus referenced dependency rows.
18. Dependency callers' raw `CallEdge`s live in `rel.dependency_calls` (the ledger needs `resolution`, `calls.rs:101-107`).
19. `from_order` is a recorded field (d17, `d17.rs:96-104`).
20. A bound's baseline is its parent commit, measured in the same session.
21. The build-counter table has a column per universe; the SCC counter sits at the top-level call sites, never inside `tarjan_scc`.
22. Every peak bound is relative (parent + 3 %); 2.85 GB reported.
23. The ordering cell fills right after the pass; subjects = reportable roots carrying an ingredient.
24. One copy of every inherited fact (`Arc<[CapabilityFact]>` per subject; one `ViewTree`).
25. Interim key = the model's string id until step 11.
26. E16 has two bounds with stated constants (4 GB, 0.8).
27. `ResolvedProgram` splits into `ProgramMeta` (lives) and `ProgramBody` (released at today's drop point); `model`, `rel`, `caps`, `identities` released after the last compute.
28. The fingerprint query/human path filters by role itself (`fingerprint_query.rs:659`, `:713-730`).
29. Cross-app failure classification from `InputSet` plus the cross-app build's own `Result`; variants and exit codes stay — **the no-id row keeps a build** (R6-3, child spec §4.4).
30. The non-convergence warning is gated by each job's own mask.
31. Cells that recurse over source build on the big-stack pool. `IN_CELL_BUILD` withdrawn (child spec review B5, 2026-10-18): the types forbid a fill from a detector; one `!in_detector_loop()` assertion at `prepare`'s entry remains.
32. The membership interner, sets and fold memo are build-local.
33. Every peak bound is two numbers: process peak and post-attach phase peak.
34. Step 7 is two steps, each bounded on its own sub-phase timer — absolute lines superseded by 43, 44.
35. Trees live until the model is built; `ProgramBody` released right after the model, before `rel` (`program_calls.rs:846-855`, `:669-676`).
36. The thread budget is `RAYON_NUM_THREADS`; `--threads` and `build_global` dropped (`big_stack.rs:66-70`; rayon-core reads the variable for every pool).
37. Step 10 carries a wall and a peak bound — wall form superseded by 45.
38. The three ordering bits key on `op` alone with `map_op`'s vocabulary (`digest.rs:56-75`; `Hyperlink` and rid-less writes).
39. `ExecRelation` carries two payloads: `typed` for `Cone`, `combined` for `Primary` (`combined_graph.rs:37-48`).
40. `query`'s job demands `CORE_SUMMARIES`.
41. Policy's switch to the pass carries an old-vs-new oracle and a zero CDO policy delta.
42. One `pd` arena, in view identity, read only by the effects cells, released at item 6 — **re-scoped by G-1 (the resolved own entries) and G-4 (lands at step 6)**; the step 6 child spec closes it.
43. `rel.typed` keeps today's concatenation order; sorting is the adjacency's and the serializer's job.
44. Every wall bound is a saving against a parent-commit sub-phase median; the one absolute line is `bfs` ≤ 1 s — **the bfs timer is split into walk and fold at step 0 (G-3); 7a's bound reads the walk half** (child spec of 7a).
45. Step 10's bound reads the `b3.adapter.join` sub-span; step 11 carries `by_decl`.
46. `analysisGaps` is its own key class (reachability, both directions).
47. Policy's rule engine is an item 8 projection off `det`; `l4` released at item 9. From step 6 on. Before step 6 it runs after `det` and `l4` are released, off the command-local `SUMMARIES | RAW_INHERITED_FACTS` build (Decision 50; child spec §4.4.4; round 9).
48. The superset-mask parity test compares the whole `DetectorOutput` — **all four fields, `d1_cohort_index` included** (R6-2; `registry.rs:255-271`).
49. One writer in the binary; a write failure prints one stderr line and exits with the command's own failure code (analyze 2; every other command 1, as today — 2 already means "no roots" on digest and "unresolved" on prove; child spec §4.2.5, review log A4).
50. Before step 6 the shared union never carries `RAW_INHERITED_FACTS`; an early batch refuses `policy check`.
51. Fallback rung "serialize fingerprint's tree before `det`" withdrawn; the ladder names the miss, then streams.
52. Step 6's stop rule reads three full-graph timings on a measurement-only path.
53. **(round 6) The epic holds contracts; each delivery slice gets a child spec written against the code as it then is.** Five rounds of review on late-step detail (G-1 to G-5 were all in steps 6, 7a, 10, 11) showed that detail written before the early steps land is re-litigated, not refined.
54. **(round 6) Per-timer noise bands, and the "≥ 2 × band" rule before a sub-phase bound is written** (I6; G-5). Step 0 records the bands.
55. **(round 6) `edges`/`dependency_body_edges` are named for what they hold, and the concatenation order is kept because the `graph` projection's tie rule is order-sensitive** (R6-5; `project.rs:277-291`).
56. **(round 6) At step 2, digest and prove compose their snapshot AFTER `det` is released**, so the snapshot never meets `det` (G-2, step-2 half; child spec §4.4). Amended in Round 13: `l4` (bundle, SCC) does overlap the composition, since the bundle is read after it (`DbTempStates`); that overlap is new for digest and prove, is sized by the `l4` bucket and is bounded by the step-2 peak bound (child spec §4.4.4).

## 9. Delivery order

Order: **0, 1, 1b, 2, 3, 4, 7a, 7b, 10, 11, 5, 6, 8, 9, 12, 13.** Byte-neutral steps
first; identity work (10, 11) before the semantic steps (5, 6) so cross-app fingerprint
lands with step 10's measured headroom. Step numbers are stable names. Every step runs the
task gate (`scripts/ci-steps task`) and `cargo test -p al-sem --lib`; steps 4–9 also run
`scripts/cdo-gate` and `cdo-measure` before merge. A bounded step measures under §5.12 and
I6 and fails on a miss. Each step is its own child issue and merges alone.

Steps 0, 1, 1b, 2 are specified in full in the child spec; the rows below are their
contract. Later steps are contract only.

| Step | Scope (contract) | Acceptance | Proof | Goldens |
|---|---|---|---|---|
| **0** Measurement | timers (`pd_solve`; BFS walk/fold split; singleton cone/own/open entry split per class; `solve_bytes` link-graph/working-set split; `b3.adapter.*` sub-spans), build counters (`build_combined_graph`, `compose_cone_over_graph`, `compute_summaries_v2*`, top-level `graph_scc`, `pd_solve`, `detector_context`) readable from the trace, `peak.post_attach`, a byte census (program parts, `InputSet`, model, base, context parts, snapshot, arena), the full-graph snapshot timings for step 6's stop rule, per-timer medians AND noise bands, the ordering counters | numbers file under `docs/`; outputs byte-identical census on/off; a `tests/cli` counter test pins today's counts per command and universe | CDO SARIF compared census on/off; one discrimination per counter | none |
| **1** Frozen inputs | `InputSet`; every post-build disk read takes it; `has_object` replaces the second parse; one stdout/file/stderr writer with one text on every command (analyze exits 2, the eleven other sites keep exit 1; Decision 49) | 8 golden targets + lib byte-identical; CDO histogram SHA unchanged; disk-edit-after-read tests, one per input class; closed-pipe test | one discrimination per restored disk read | none |
| **1b** `ResolvedProgram` | retained program, two edge sets apart, `graph` reads it, `export_workspace`'s private build deleted; body released at today's point | `alsem graph` byte golden; the body dropped at today's `b3.ctx_drop` span (`program_calls.rs:674-676`; `body.is_none()` after a `take()` is true by construction and pins nothing, child spec A1b.2 / review A7) with the post-attach peak as the measurement; both peaks ≤ parent + band on CDO analyze and digest; `InputSet` bytes in the census | drop `dependency_body_edges` / `site_facts` from the projection input → `graph_export_covers_dependencies` fails | none |
| **2** Owned artifacts, borrowed context, one context per command | `DetectorArtifacts` + `L4Analysis` + borrowed `DetectorContext<'v>`; driver-owned `View` with `prepare`/`release`, `!in_detector_loop()` at `prepare`; ordering cell; `det` at the union of masks (policy's RAW build a command-local, not a cell, until step 6: I5, Decision 50), parity test over the whole `DetectorOutput`; digest/prove/events/policy/query reuse; prove `Some(root)`; analyze's failure classification; the warning on `l4` | counters read the child spec's table exactly; goldens byte-identical; CDO digest saving ≥ parent median of the deleted context's span (`digest.tx_context_map`, step 0) − its band − band(digest wall) (I6; the saving is read off two whole-command walls, so their noise is subtracted too; ≈ 21 s on P5, indicative only); both peaks ≤ parent + band for analyze, digest and prove; cell census at the first run | one discrimination per counter; `Some(root)` moves no cli-b prove golden | none |
| **3** Fold from uncertainty edges | per-routine scan replaces the snapshot's fold solve; still only where the fold applies today, over the cut graph | r3a3, cli-b, r4f byte-identical on stdout; −1 `compute_summaries_v2*`, −1 `graph_scc` per snapshot; stderr: one non-convergence line fewer on a capped SCC, stated | oracle old == new on r0, gap, CDO with all four kinds in an SCC and a singleton; two discriminations (old kind list; new scan's kind list against hand-stated records). As delivered (2026-10-20): the gap half was substituted by the 32 fixture dirs plus hand-stated inline fixtures, since `tests/gap`'s inline scenarios cannot reach a test-only lib oracle (see the CHANGELOG's #160 step 3 entry) | none |
| **4** `ExecRelation` | `Transfer::{Cone, Primary}`, two payloads, eager adjacency/SCC; base, context, r3a5, L4 read §5.5's table; snapshot on `LegacyCut`; dependency uncertainty edges kept | byte-identical everywhere; CDO detector baseline identical; counters: `build_combined_graph` 1, `graph_scc` 2 per view (+ snapshot's own until step 6) | dependency-publisher fixture (span, temp proof, reverse callers unchanged; discrimination "Primary callers only"); two-caller `from_order` fixture (reverse it; d17's sample flips) | none |
| **7a** Membership: one inherited set per SCC | `DerivedOnly` reads one set per Cone-SCC; old path test-only until CDO passes | goldens unchanged; `bfs.walk` ≤ 1 s on CDO (absolute floor; expected ≈ 0) **and** cone saving ≥ parent `bfs.walk` median − 1 s − band(bfs.walk); retained ≤ 98 MB. The fold half (`bfs.fold`, G-3) is NOT in 7a's bound unless `fold_row` moves into 7a; the child spec states which | derived-row oracle on r0, gap, r4, CDO; scaling fixtures (chain, fan-out, 479-SCC, guard-heavy, producer blow-up); stop rule from step 0's SCC distribution | none |
| **7b** Membership: closed-class sets, fold memo | `FactKeyInterner`, hash-consed `SetArena`, closed/open split, per-`SetId` memo, `fold_row` | goldens unchanged; (scan + fold + set-building) saving ≥ X − band and cone saving ≥ X − band, X re-derived at step 0 from the cone/own split and written down before code; X ≥ 2 × band or the step is dropped | 7a's oracle; a shared-callee-set fixture where the census counts one fold (disable the memo → two) | none |
| **10** `SiteIx`, delete the span join | `SiteIx` on edges, call sites, ops; join by key with the span as a debug assertion; then delete J1, `texts`, J4 | `losses() == []` both universes on r0 + CDO; `edge_order_is_call_sites_then_triggers` green; attach saving ≥ parent `b3.adapter.join` − band(join) (only if join's step 0 median ≥ 2 × band, else a non-speed step); both peaks ≤ parent + band | debug span assertion on the whole suite before deletion; mis-pair one dependency site → the loss gate fails | none |
| **11** `RoutineIx` | dependency rows included; string ids a projection; J2, J5 deleted; the collided-id rule | byte-identical; id-pattern counts per golden dir unchanged; CDO collision delta reported; attach saving ≥ parent `by_decl` − band (same ≥ 2 × band rule). **The `pd` arena is NOT this step's** (G-4) | hand-stated collision fixture asserting today's three rules on old code and the one rule on new code (make it first-wins → the union assertion fails) | none |
| **5** One coverage policy | §6.3 in context, base, r3a5, L4, both universes, dependency uncertainty edges included | context == snapshot coverage per root on r0; on the dependency fixture the context's fold checked against its own `Cone` relation (the snapshot is still cut until 6); CDO triage of every span/coverage reader and of `policy check` with the owner's policy; > 30 % FP = opt-in or root-cause fix | ambiguous-call fixture (`partial`, span `coverage_complete=false`; remove the fold → fails); policy fixture with both `on_unknown` outcomes hand-stated | r3a5, cli-b digest/prove, cli-c-events, cli-c-policy, r4, cli-a, gate, `cli_d35_unknown_commit`; `cdo-measure` per detector |
| **6** One cone graph + scoped witness pass | snapshot on `Transfer::Cone`; `WitnessScope`; the `pd` arena in view identity (moved here from 11) and the resolved-entries decision (G-1); effects cells; `identities` cell; per-job trees with the retention decision (G-2); `RAW_INHERITED_FACTS`, `ConeOutput::Both`, `inherited_raw` deleted; `LegacyCut` and `build_r3a3_source_only_base` deleted; `unfetched-dependency` ledger rule; R6-4's serialization filter and link rule | #161 acceptance; context == snapshot coverage on the dependency fixture; CDO digest wall and both peaks ≤ parent + band; same for analyze with d47/d49/d51; counters at the final row (1 per view, `graph_scc` 2, `pd_solve` 1 or stated 2, witness pass 1 or 0); no raw `Vec` for any dependency subject; CDO `policy check` moves nothing; stop rule from step 0 (iv)'s three timings plus the solve | `mk.py` test (revert to cut → fails); scope discrimination; scoped-equals-unscoped oracle (digest every `c.diff` root, prove, fingerprint, ordering) with the whole-routine-gate discrimination; ordering root-set oracle with the `Hyperlink` and rid-less fixtures; policy facts oracle; `--diff` root resolution byte-equal; no-dependency parity | cross-app digest expectations in `cli_digest_query_dependencies`; `r4_cross_app_program` if it covers d47/d49/d51; CDO deltas triaged |
| **8** Roles, schema 4, preflight | `Role` with the `dep:` filter beside it; live `is_dependency_role` with reachability; key classes; schema 4; `apps[]` by role; preflight | every cli-b diff against an allow-list (header, `apps[]` roles, gap role annotation; anything else fails) | reachable-only gap test; mixed-universe refusal; shards with a dependency `apps[0]`; one test per key class, gaps in both directions | every cli-b file; `cli_p1_inventory`; `cli_workspace_fingerprint_closure`; the key list at `tests/differential.rs:1758-1799` |
| **9** fingerprint, prove, diff cross-app | the cross-app `View`; `--single-app`; three stated output moves (diagnostics source, dependency shards, `unresolved_cone` caps) | fixtures; no-dependency parity; CDO fingerprint time/size reported; both peaks ≤ parent cross-app analyze + band; cell census; a miss walks §5.12's ladder | fixtures (a) two-hop not PROVEN, (a') one-hop with ledger + gap, (b) inherited fact through a dependency, (c) dependency-mediated `capability-gained`, (d) query path by role; one discrimination per filter | cli-b header `cross-app`; contents unchanged |
| **12** `alsem batch` | plan; one view per (workspace, universe, `--no-roots-config`); fixed job order; one pass at the union; `JobOutput` writes; §5.11's batch rules; no `--threads` | per job: stdout, stderr lines, exit, files equal standalone; one build/`det`/cone/pass per view; the `Arc` census; CDO analyze+digest+fingerprint batch < standalone analyze + digest; each peak ≤ 1.15 × the largest standalone; four workers under E16 | contract test over every command; split traces per job; separate passes → counter 2 fails. Early batch after step 2: ≤ 55 s, two views, refuses `policy check` (Decision 50) | none (new family) |
| **13** Incremental host | generation-checked cancellable L4 work off `on_swap` (`src/lsp/updater.rs:1206-1229`); invalidation to reverse reachability, SCC, demand, config files; watcher blindness fixed (`src/watcher.rs:29-42`); salsa adopt or remove | incremental == fresh after body, signature, edge-removal, SCC split/merge, dependency edits | — | none |

## 10. Acceptance matrix for the epic

| # | Claim | Evidence | Step |
|---|---|---|---|
| E1 | One context, cone, combined graph, solve, `pd_solve`, witness pass per view | step 0 counters, one discrimination each; step 2 states the remaining duplicates; step 6 the final row | 2, 6, 12 |
| E2 | Byte-neutral steps move no golden | 8 targets + lib | 0–4, 7a, 7b, 10, 11 |
| E3 | #161 closed | `mk.py` test + coverage equality | 5, 6 |
| E4 | One coverage policy, every reader listed | fixtures: span, fanout, d35, d45, policy gate | 5 |
| E5 | Inputs frozen, one edit per input class | disk-edit-after-read tests | 1 |
| E6 | Cross-app prove never falsely PROVEN | fixtures (a), (a') | 8, 9 |
| E7 | Behavioural fingerprint/diff cross-app; every key classed | fixtures (b), (c); one test per class | 8, 9 |
| E8 | Mixed universes/schemas refused | preflight test | 8 |
| E9 | No-dependency parity for every command | parity tests | 6, 9 |
| E10 | Cone faster, same rows, each sub-phase bounded on its own timer and band | oracle; `bfs.walk` ≤ 1 s and cone saving (7a); X-bounds (7b) | 7a, 7b |
| E11 | Adapter gone, no losses, collided ids under one rule | `losses() == []`; collision fixture | 10, 11 |
| E12 | Memory within budget; no dependency subject materialized; `det` never carries a raw fact | both peaks every retention step; census | 1b, 2, 6, 9, 10, 12 |
| E13 | Batch equals standalone | contract test | 12 |
| E14 | CDO ratchets unchanged | `scripts/cdo-gate` | every non-docs step |
| E15 | Measured speed is accepted, not reported | step 2 digest saving ≥ parent `digest.tx_context_map` median − its band − band(digest wall) (the saving is a difference of whole-command walls); 7a/7b/10/11 per I6; 6 within parent; 12 batch < analyze + digest | 2, 6, 7a, 7b, 10, 11, 12 |
| E16 | Four workers fit the machine | summed peak ≤ RAM − 4 GB; throughput bounds | 12 |
| E17 | Transfer rules reproduce today's adjacency per field | §5.5 table; publisher fixture | 4 |
| E18 | One `pd` arena, never alive during a run; one write-failure exit code | step 6 census; closed-pipe test | 1, 6 |

## 11. False-positive and failure shapes

False PROVEN before §6.2/§6.4 land (step order, fixture (a)). Demand churn as false diffs
(primary-only emission, reachability-filtered gaps). Silent mixed-universe diff (schema 4
+ preflight). More `partial` coverage after step 5 (triage; > 30 % FP = opt-in). New
d47/d49/d51 findings after step 6 (triage). Output depending on fill order (`RunKey`
mask). Rayon deadlock from a lazy fill (rule 1). A dependency subject with a raw cone
(`WitnessScope`, deletion of `RAW_INHERITED_FACTS`, census). Four workers oversubscribing
(`RAYON_NUM_THREADS`, pool-size test). A pass per job in a batch (counter). Stale facts
mixed with fresh disk (`InputSet`, step 1 tests). Membership over-approximation across a
guarded edge (closed/open split, derived-row oracle). A scoped pass changing an answer
through a reader that looks past the subject (per-routine tree, scoped-equals-unscoped
oracle) or a root resolved before the pass (`identities` cell). A cone reaching a routine
with no model row must fail closed (`program_calls.rs:1097-1114`; step 9 traces it). A
vacuous sub-phase bound (I6's ≥ 2 × band rule). A duplicate `pd_solve` hidden inside the
witness pass (the step 0 `pd_solve` counter).

## 12. Carried findings

Round 6 (`scratchpad/critiques-160-r5.md`: G-1 to G-6 on memory, speed and scheduling;
R6-1 to R6-5 on byte identity and correctness) was checked in source; every finding held.
Each is assigned to the step whose child spec must resolve it. "Lineage" names the earlier
findings a round-6 finding re-raises, which means those earlier dispositions were not
complete.

| Finding | Severity | Lineage | What it says | Resolved by |
|---|---|---|---|---|
| G-1 / R6-1 | important | r1 A3, r2 A2, r5 R5-1/G1, Decision 42 | The witness pass reads the solve's resolved own entries (`capability_cone.rs:2447-2448`, `:2559`, `:2571`; `ResolvedEntry<'g>`, `pd_solve.rs:89`), which the spec drops inside `caps`; "one `pd_solve` per view" cannot hold as written; no counter sees a second solve | **Step 0** adds the `pd_solve` counter and timer (child spec §4.1); **step 6** chooses retain-in-view-identity vs re-solve and re-budgets §5.6/§5.12 |
| G-2 | important | Decision 15 | Digest's `ViewTree` (every routine's facts, edges, evidence) lives through `det` and every run, which is new retention (today digest frees its snapshot first, `digest_cli.rs:1643-1650`); no `ViewTree` size estimate; no digest fallback rung. Step-2 half: the tx map comes from `det.spans`, so the snapshot could outlive the run | **Step 2** (child spec §4.4): snapshot composed after `det` is released, so the snapshot never meets `det`; the one new overlap is `l4` (bundle, SCC) through composition in digest and prove, sized by the `l4` bucket and bounded by the step-2 peak bound (child §4.4.4, Round 13); **step 0** adds `ViewTree`-equivalent bytes (today's `CapabilitySnapshot` parts) to the census; **step 6** decides retention shape and the digest fallback |
| G-3 | important | r4 F2, r5 G3, Decision 44 | `BFS_NANOS` covers `emit_picks`'s per-pick fold (`:2551-2580`, `:2080-2084`); 7a cannot bring it to ≈ 0 without `fold_row`; the bound does not name its command | **Step 0** splits `bfs` into walk and fold with picks per call (child spec §4.1); **step 7a**'s spec sets its bound from the walk half and names the command |
| G-4 | important | Decision 42 | The view-identity arena needs `rel.typed`'s `EdgeIx` and `ViewTree.direct`'s `DirectFactIx`, which first exist at step 6; step 11 precedes step 6 | **Epic**: the arena moves to step 6 (§5.6, step table); step 6's stop rule counts its cost |
| G-5 | important | Decisions 34, 44 | The noise band is defined only for step 2; sub-phase bounds for 7a, 7b, 10, 11 can pass with no saving | **Epic** I6 + Decision 54; **step 0** records per-timer medians and bands (child spec §6); steps 7a/7b/10/11 apply the ≥ 2 × band rule in their specs |
| G-6 | minor | — | `ProgramMeta` holds `InputSet.al_files[].text`, 5.09 MB on CDO, not "a few KB"; not in the census | **Step 1b** (child spec §4.3): size stated, `InputSet` in the census |
| R6-2 | minor | Decision 48 | `DetectorOutput` has a fourth field `d1_cohort_index` (`registry.rs:265-270`, lifted at `:682-684`) the widened parity test must compare | **Step 2** (child spec §4.4, §8): compare all four fields; `D1CohortIndex` gains `PartialEq` |
| R6-3 | minor | Decision 29 | The "no-id → no build" row is not today's behaviour: the cross-app failure path's single-app rerun builds first (`run.rs:387` → `:285-291`), then checks the id (`:296-307`); `fresh` is defined by that build | **Step 2** (child spec §4.4): the no-id row keeps one build; `fresh` is its `Result`; a test pins the text |
| R6-4 | minor | — | "One row per `rel.typed` edge at the same index" omits the rows `derive_typed_edges` drops (`snapshot.rs:1395-1501`) and the `PD_EDGE_UNMAPPED` links (`:1580-1590`) | **Step 6**: serialization filter and link rule stated, anchorless-edge fixture |
| R6-5 | minor | r3 N5 | "No caller sits in both edge sets" is false (`mod.rs:222-230` only filters a histogram; publishers from every app sit in `report.edges`); the projection merge is order-sensitive on ties (`project.rs:277-291`) | **Step 1b** (child spec §4.3): names and justification corrected; order kept |

## 13. Open questions

1. SCC-member filter for the four kinds; other emitters of the kinds (step 3 oracle).
2. Is the temp class of a non-parameter-dependent fact constant along a path? (`static_members`; step 7a reads `pd_solve.rs` in full.)
3. How much of the compose remainder is `pd_solve`? (step 0.)
4. How common are frame-bound entries on CDO? (step 0.)
5. ~~Do digest/prove read only inside the roots' closure?~~ No (§5.7; Decision 14).
6. Is demand a superset of cone reachability? (`full.rs:2160-2164`; step 9 fail-closed trace.)
7. Does every op the body walk numbers have an `ExprId` the extractor sees? (step 10.)
8. Which goldens beyond d35's move under step 5?
9. ~~Does `T → &T` auto-deref cleanly per detector after step 2?~~ Closed by construction in the child spec (§4.4: `Deref` to the artifacts keeps every field name and type).
10. Retained sizes; cross-app fingerprint size, time, peak (steps 0, 9).
11. `--no-roots-config` as an overlay on one view? Kept separate.
12. What does `resolve_changed_roots` read besides `snap.identities`? (step 6 byte check.)
13. (round 6) Does `caps` retain the resolved own entries, or does the pass re-solve? (step 6, with step 0's numbers.)
14. (round 6) Which `ViewTree` parts does a digest-only plan keep through the runs? (step 6.)
15. (round 12) `InputSet` (§5.1) as specified for steps 0-2 also carries request inputs (policy, baseline, hash rows); a batch view keyed by workspace cannot share it across jobs with different `--policy`/`--baseline`. Step 12 splits request inputs from workspace inputs (child spec §10 item 5).

## 14. How this spec was chosen

Two designs were judged: A (session-first) and B (data-first). **B is the spine:** named
transfer rules as data; byte-neutral `ExecRelation` first, then the snapshot flip; no
`RevisionKey`; reachability-filtered gaps; a concrete `RoutineIx`/`SiteIx` plan; the honest
cross-app-fingerprint trade-off. **Grafted from A:** the cell rules (tightened to a
driver-owned `View`), the transport, the frozen-input proof, the cone derivation detail.
Dropped: A's early cross-app loss gate (step 4b).

## Appendix: Review log

Every finding was checked in source before the spec moved; no round refuted one. Each
row: disposition and where the fix lives. Rounds 1–5 moved the single document; round 6
produced this restructuring (Decision 53) and §12.

**Round 1** (`critiques-160-r1.md`; A = memory/speed/scheduling, F = byte identity): 29
findings, 29 fixed.
A1 critical: `RawOnly` over every routine is the 10.9 GB shape → `WitnessScope` in step 6.
A2 critical: `nodes` is universe and walk set → subject set, `All` dropped.
A3 important: digest frees its snapshot before its run → one pass, fixed order (re-raised r2 A2, r5 R5-1, r6 G-1).
A4 important: `big_stack_pool()` sizes to every CPU → thread budget (superseded by 36).
A5 important: lazy `OnceLock` deadlock → built-input builders, `IN_CELL_BUILD`, eager SCC.
A6 important: second parse tests only `objects.is_empty()` → `has_object` bit.
A7 important: no wall-time claim → bounds (E15).
A8 important: cone census populations → membership tables, stop rule.
A9 minor: identity steps after semantic → order 10, 11 before 5, 6.
A10 minor: `OnceLock::take` needs `&mut` → driver-owned `View`.
F1 critical: reverse graph inverts `edges_by_from` incl. every event dispatch → `Primary` rule, field table (sub-claim corrected: `find_reachable_roots` takes no edges).
F2 important: coverage readers (spans `TX_UNKNOWN`, fanout, d45) → §6.3 cost, step 5 goldens.
F3 important: roots resolve from `snap.identities` → two-phase snapshot.
F4 important: inventory lifts `coverage` → kept (Decision 9).
F5 important: `Walk` vs `Skip` lists → `al_files` with `nested_app`.
F6 important: unassigned snapshot keys → key classes.
F7 important: late validation order → stated precedence changes.
F8 important: `compose_snapshot`'s cone is not `context.capability_cones` → build counters.
F9 important: three collision merge rules → one rule.
F10 important: symbol-only call resolves to the ABI row → ledger rule + gap.
F11 important: `graph` needs dependency trees, merged edges, site facts → step 1b.
F12–F19 minor: parity licence vs `ALL|RAW`; fold changes records only; oracle carries four kinds; one edit per input class; `apps[]` allow-list; step 9's three output moves; cross-app failure rerun stated; `dep:` filter kept.

**Round 2**: 21 findings (19 distinct), 21 fixed. B1 critical: the walk gate skips
non-members as a whole and readers look past the subject → per-routine tree, oracle
(Decision 14). B2: identity table read for display → unfiltered in memory (17). B3/A3:
ordering prewarm → pass before `det` (15). B4/A4: `ALL|RAW` composes `Both` over every
routine → bit deleted (16). B5: ledger needs `resolution` → `dependency_calls` (18). B6:
d17 reads emission order → `from_order` (19). B7–B11 minor: object roles; shard-filter
citation; `inputs` class; `WsFile.text: Result`; second discrimination for step 3. A1:
per-command counter table. A2: pass moved before `det`, `pd` released. A5: snapshot on
`LegacyCut` until 6. A6–A10 minor: pools; parent-commit baselines (20); `graph` job first;
guard restated; step 2 bound as a saving.

**Round 3**: 17 findings (16 distinct), 17 fixed. N1/I1 (r2 A1 re-raised): base builds two
graphs and one SCC → table per universe, `graph_scc` at top-level sites (21). N2: fingerprint
query path unfiltered → role filter (28). N3: policy gate under fail-closed replaces the
finding (sub-claim corrected) → step 5. N4: `build_cross_app_base` returns `Option` →
classification from the build's `Result` (29; corrected again by R6-3). N5–N7 minor: step 1b
discrimination; `parenless_calls` and both histograms retained; warning gated by mask (30).
I2: relative peak bounds (22). I3: ordering cell at item 6 (23). I4: one copy per fact (24).
I5: interim string key (25). I6: E16 bounds (26). I7: `ProgramMeta`/`ProgramBody` (27).
M1–M3 minor: wall time in step 0; big-stack cells (31); build-local membership (32).

**Round 4** (`critiques-160-r3.md`): 10 findings, 10 fixed. F1: post-attach phase peak
(33). F2: step 7 split with sub-phase bounds (34). F3 (r3 I7 re-raised): trees live until
the model (35). F4: `RAYON_NUM_THREADS` (36). F5: step 10 bounded (37). R4-1: ordering bits
on `op` (38). R4-2: two payloads (39). R4-3: query demands `CORE_SUMMARIES` (40). R4-4:
policy oracle (41). R4-5: `abs_path` kept.

**Round 5** (`critiques-160-r5.md`, first half): 13 findings (12 distinct), 13 fixed.
R5-1/G1 (r1 A3, r2 A2 re-raised): one `pd` arena in view identity (42; re-opened by r6 G-1,
G-4). R5-2: `typed` concatenation order (43). R5-3: `Gap` class (46). R5-4: policy engine
at item 8, `l4` released at 9 (47). R5-5: whole `DetectorOutput` (48; completed by R6-2).
R5-6: one writer, exit 2 (49). G2: own entries miscounted → step 0 split (44). G3 (r4 F2
re-raised): savings against parent sub-phase medians (44; re-opened by r6 G-3, G-5). G4:
adapter sub-spans (45). G5: three full-graph timings (52). G6: early batch refuses policy
(50). G7: fallback rung withdrawn (51).

**Round 6** (`critiques-160-r5.md`, second half: G-1 to G-6, R6-1 to R6-5): 11 findings
(10 distinct; G-1 and R6-1 are one), all held in source, none refuted. Disposition: §12
(carried to the step that must resolve each) and Decisions 53–56. Epic restructured; the
steps 0–2 child spec resolves G-1 (instrument half), G-2 (step-2 half), G-3 (instrument
half), G-5 (bands), G-6, R6-2, R6-3, R6-5; G-4, R6-4 and the design halves of G-1, G-2,
G-3 go to step 6 / 7a.

**Round 7** (reviews of the steps 0–2 child spec, 2026-10-18; child review log rows
A1–A12, B1–B14, C1–C12, D-I1–D-M7): five epic-level amendments, each a verified finding,
none a new decision. Decision 49 and §5.10: exit = the command's own failure code (child
A4; "two sites exit 1" was a miscount, eleven). §5.9 rule 1 and Decision 31:
`IN_CELL_BUILD` withdrawn (child B5). §5.8: `L4Harvest` apart from `L4Analysis` (child
B8). Step-1b row: `body.is_none()` replaced by the drop span plus the peak (child A7).
§5.9 rule 4: census counters never reset (child D-M6). §5.9's `View.program` annotated
as joining at its first reader (child D-I3). Policy's RAW build before step 6 is a
command-local, not a cell, which keeps I5 and Decision 50 as written (child C3).

**Round 8** (child review log round a2-r1, 2026-10-18): 15 findings, all held in
source, none refuted; one epic-level amendment. §5.8: the transient `L4Harvest` is
replaced by the unharvested `CoreSummaries` map, and the harvest runs in the `det`
builder (child a2-r1 2-8: query paid a harvest it never reads, and the harvest needs
`uncertainty_edges_by_from`, which only the `det` builder has). The other fourteen
are child-local (counts, tests, `release(Graph)`, ownership types).

**Round 9** (child review log round a2-r2, 2026-10-18): two reviews, 23 findings (21
distinct), all held in source, none refuted; one epic-level amendment. §5.9 rule 3
item (8) and Decision 47: before step 6 the policy rule engine is not an item-8
projection off `det`, since it reads raw inherited facts that no `det` may carry (I5,
Decision 50); it runs after `det` and `l4` are released, off policy's command-local
RAW build, and moves to item 8 when step 6 deletes `RAW_INHERITED_FACTS` (child a2-r2
R1-9). The other findings are child-local (probe gating, the `InputSet` parameter
path, the cross-app failure enum, cell fill-once asserts, tests).

**Round 10** (child review log round a2-r3, 2026-10-18): two reviews, 16 findings (15
distinct), all held in source, none refuted; one epic-level amendment. §5.2: the app set,
ABI integrity and the build profile leave the step-1b `ProgramMeta` until a step reads
them, since no step 0–2 reader does and `AppMeta` had no definition (child a2-r3 S9).
The other findings are child-local (the walk error kept as an `io::Error`, the census
residual taken from `tools/census-probe`, one builder form for steps 1b and 2, the
full-graph timing inputs, the ordering fill first and on the big stack, `build_l4` as
the one warning print site, a `build_only` read for the LSP, the transient-test ledger,
the unreadable-file recipe, test lists).

**Round 11** (child review log round a2-r4, 2026-10-18): two reviews, 13 findings (11
distinct), all held in source, none refuted; three epic-level amendments. §5.9 rule 3
item (7): the `det` builder runs its cone, then the `l4` solve, then the harvest, as
today; an `l4`-before-`det` order would have kept the bundle and core map alive through
the cone (child a2-r4 1). I2: a byte-neutral step may delete or rewrite a `--lib` test
that pins the code it deletes, when the child names it (child a2-r4 M4). The base
worktree is `…-a2` (child a2-r4 M4). The other findings are child-local (a test seam
for injected diagnostics, a census mark and text ownership, the `EdgeView` signatures,
the full-graph mode's step-0 expression, digest's no-input check order, signature
fallout, guard needles, spawned-process tests).

**Round 12** (child review log round panel-r1, 2026-10-18; reviewers sol and opus): 17
findings, 15 distinct, all held in source, none refuted. Epic amendments: §4 names the
two causes apart (lifetime for #160; per-consumer relation and fold for #161); the step-2
row and E15 bound digest's saving on the parent median of the deleted context's span
(`digest.tx_context_map`) minus its band, per I6, not on the P5 "21 s"; the step-2 peak
bound adds prove (its `l4` now lives through snapshot composition); open question 15
records that `InputSet` carries request inputs, for step 12. The rest is child-local.

**Round 13** (child review log round panel-r2, 2026-10-18; reviewers sol and opus): two
re-raised entries (P1-7, P1-10) and five new ones (P2-1 … P2-5), all held in source,
none refuted. Epic amendments: I6, E15 and the step-2 row subtract the digest wall's own
band too, because a deleted sub-phase's saving is read off two whole-command walls
(P1-7); §5.8 drops the field index from `L4Analysis`, and §5.9 rule 3, Decision 56 and
the G-2 row state the `l4` overlap during snapshot composition in digest and prove,
which Round 12 fixed in the child only (P2-1). The rest is child-local.

**Round 14** (child review log round panel-r3, 2026-10-18; reviewers sol and opus): all
20 entries accepted; three new minor findings (P3-1 … P3-3), all held in source, none
refuted. Epic amendment: I6 withdraws the "8 % of the command median" band for every
bound, so a whole-command wall's band is max − min of the parent's wall runs, as the
child already uses (P3-2). The rest is child-local.
