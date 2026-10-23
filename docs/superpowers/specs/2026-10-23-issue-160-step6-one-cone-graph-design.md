# #160 child spec: step 6 — one cone graph, one scoped witness pass

Status: written for spec panel round 1 (2026-10-23). Made from the pre-draft
`/work/bench/160-6/draft-step6-design.md` (master `5c42b8f3`) once the base probes had run;
§0 lists what the probes changed.

Child of `2026-10-18-issue-160-analysis-session-design.md` (the epic): §1 goals 2, 6; §5.5
(M3, M5, M8), §5.6, §5.7, §5.9 rule 3, §5.10, §5.12; §9 row **6**; E1, E3, E9, E12, E15, E18;
I1-I10; Decisions 2, 8, 14, 15, 16, 18, 20, 22-24, 33, 41-43, 47, 50-52, 56, 58; carried
findings G-1, G-2, G-4, R6-4; open questions 12, 13, 14. The shape follows
`2026-10-22-issue-160-step5-coverage-policy-design.md`. Where this document and the epic
disagree, this document is wrong, EXCEPT the numbered amendments of §9 (6A-1 to 6A-10). Each
gives its reason; until the epic takes it, it is an open question for the panel.

Base: master `459294f1` (step 5's merge, #223), worktree `/work/al-sem-issue-160-a7`. Every
`file:line` below is in that tree. Every fact the design rests on is a row of the ledger's
`## Assumption probes` table (`.agent/issue-160/ledger.md`). The scripts and raw output are in
`/work/bench/160-6/` (`probe-base.sh`, `probe-base2.sh`, `probe/`); the base binaries are
`/work/bench/160-6/parent/` (`bins.sha`). Nothing from the probes is on the branch.

**Ordering constraint: #220 (epic step 8) merges BEFORE this step's implementation.** Both edit
`src/engine/l5/snapshot.rs` and `snapshot_full.rs`. Every citation marked **[#220]** below is
in a file #220 edits (`snapshot.rs`, `snapshot_full.rs`, `capability_cone.rs` near
`build_cross_app_base`, `exec_relation.rs`, `digest.rs`, `selector_index.rs`, `registry.rs`)
and is re-cited at the plan, on the post-#220 master. §11 lists what #220 hands this step.

Words:
- **Universe** = every routine row of the view. **Subject** = a routine whose raw inherited
  facts some job reads. **`WitnessScope`** = the subject set.
- **Cut graph** = the snapshot's own `build_combined_graph` over `model.calls`
  (`capability_cone.rs:4505-4513`), which cross-app holds primary callers only ("legacy cut",
  epic M9). **Cone graph** = `rel.cone()` (`exec_relation.rs:111-114`): the primary half's
  typed edges with event dispatch, then the dependency half's.
- **Raw** = a routine's `Vec<CapabilityFact>` of inherited facts. **Derived** = its
  `ConeDerivedStore` row. **pd** = the producer solve's `ConeArena`
  (`capability_cone.rs:2640`).
- **Tree** = the view's `CapabilitySnapshot` (`snapshot.rs:881`) **[#220]**. **Effects** = the
  per-root digest entries and the ordering facts computed from the tree.
- **The bound commands** (epic row 6): `d` = cross-app `digest <WS> --file
  src/DocumentTypes/Finance/General/CDOIssueDocument.Codeunit.al --format json`; `o` =
  cross-app `analyze <WS> --detector
  d47-io-unsafe-txn,d49-uncommitted-write-before-ui,d51-retry-side-effect-duplication
  --format sarif`.

## 0. What the probes changed in the pre-draft

1. **A6.4 is infeasible as worded, decided before any code.** `o` builds no det cone today
   (its `cone 1` is the cut snapshot's, `cli_build_counters.rs:233`), so nothing can be fused:
   the head must compose once over the full cone graph where the parent composed over the cut.
   Measured at the base on CDO: one full-graph composition is 6,136 ms
   (`context.capability_cones`, cross-app default analyze; `pd_solve` 2,788 ms, fact cones
   1,543 ms), the cut's is 677 ms (`r3a3.compose_cone`; `pd_solve` 74 ms). That is ≥ +5 s on
   `o`'s 17.4 s median before any raw walk or extra ordering root. The bound is re-stated as a
   pre-stated cost bound (6A-10).
2. **Policy violates I5 today.** Its RAW build composes raw for EVERY routine on the cone graph,
   dependency rows included: the base peaks at 14.3 GB (custom policy) and 13.2 GB (bundled),
   and the raw half costs ≈ 24 s of its 36.6 s of cone time (two compositions; the derived-only
   one is ≈ 6.1 s). Scoping it to primary subjects is the step's largest memory and time win.
3. **The void check is fragile on a shared server1.** At the base, with a load spike in one
   round, `d`'s band is 2.1 s (9.8 % of 21.6 s) and `o`'s 1.4 s without the spike round (8 %),
   so the pre-draft's "band ≤ 5 % of the median" would have voided both. 10+11a measured `d`'s
   band at 0.2 s in a quiet slot. The rule is kept; §8 runs Part 2 only in a quiet slot, and
   F9 is the stated outcome of two noisy runs.
4. **The cone is still keyed by string id after 11b** (`capability_cone.rs:2585`), so
   `WitnessScope` holds string ids (Q7 closed).
5. **The ordering engine reads `callsite_resolutions` by caller** (`ordering_engine.rs:187-192`),
   so dependency callers' ledger rows (D6) are needed by `o` on the cone graph, not only by a
   serializer.
6. **#220 delivers the gap half of Decision 8** with `rel.cone()` reach; 6A-4 becomes a
   statement of fact, and §11 lists the hand-over.
7. **prove, fingerprint and diff stay on a standalone composition** (6A-9): single-app only at
   step 6, so moving them now buys nothing step 9 does not redo.
8. P1-P4 head halves, P6 dynamic and P8 need the scope type; they are Task 1's measurements and
   the stop rule's inputs (§8.4). P5, P9, P13 hold at the base.

## Feasibility first

Speed step (§5.12's peak list). Measured on server1 (22 cores, 93 GB, shared with another
session).

| Item (row 6) | Verdict | Evidence |
|---|---|---|
| Unscoped tree on the cone graph | not feasible, not needed | step 0: 31.4 GB peak (Windows); the tentacle limit is 32 GB. `WitnessScope` is mandatory |
| One composition per view where a det cone exists (digest, policy, analyze with summaries) | feasible by construction | §4.2: the det composition gains a raw half; nothing is retained across cells |
| #161 acceptance; context == tree coverage | feasible | T1, T8: one fold (step 5), one composition |
| **A6.3 `d` wall ≤ parent + band** | **likely**: the head deletes the cut base (952 ms: graph 71, clones 22, direct scan 201, compose 650) and adds the raw walk for 3 roots plus a derive over the full typed edges (P3 head, Task 1) | probe P3 base half |
| A6.3 `d` peaks | process peak likely (3.15 GB, set in attach); the post-attach peak is not readable on Linux (#204, 6A-8) | P7 peaks |
| **A6.4 `o` wall ≤ parent + band** | **not deliverable as worded, because** the parent composes over the cut and the head must compose over the full cone graph: ≥ +5 s (§0 item 1). Delivered as 6A-10's cost bound | P4 base half, the full-graph composition timing |
| A6.4 `o` peak ≤ 1.03 × parent | unknown → P1: the full-graph composition fits under the attach peak in default analyze (3.07 GB); what is new is the raw of 5,479 reportable roots and the cone-graph tree. Ceiling: every routine's raw is 9.4 GiB (step 0) | §8.4 S2 |
| counters at the final row | feasible | §4.9 |
| no raw for a dependency subject | feasible by construction; fixes policy's violation | D3, T8 |
| CDO `policy check` moves nothing | feasible: same composition, same facts for the primary routines it evaluates | P10 SHAs |
| stop rule | defined in §8.4, read after Task 1 | — |

## 1. Goal

1. **One cone graph.** The tree is composed over `rel.cone()`. The cut graph's builder
   (`build_r3a3_source_only_base`, `_over`, `R3a3SourceBase`, `capability_cone.rs:4476-4609`)
   and the measurement-only `aldump --issue160-full-graph-timings` (`:4611-4680`,
   `aldump.rs:61-70`, `:223`) are deleted. This closes #161: a workspace routine's digest
   effects include what its dependency callees do, and the tree's coverage is the context's
   (step 5's limit 5-A3 closes).
2. **One composition per view.** Where a plan has a cone today, the det cone and the witness
   pass are ONE `compose_cone_over_graph` call: derived rows and coverage for every routine of
   the view, raw facts only for the subjects, one `pd_solve` (G-1: neither retain nor
   re-solve).
3. **Scoped witnesses, never for a dependency subject (I5).** Subjects are primary routines.
   `RAW_INHERITED_FACTS`, `ConeOutput`, `inherited_raw()` and policy's command-local RAW build
   are deleted; policy reads the pass (Decision 16).
4. **The call-site ledger reads every caller** (`model.calls.edges`, then
   `rel.dependency_calls`, Decision 18), and a call resolved only into body-less dependency
   rows is `unfetched-dependency` (Decision 8, ledger half; the gap half is #220's).

## 2. Non-goals

- **fingerprint, prove, diff** stay single-app (step 9) and stay on a STANDALONE composition
  (`compose_snapshot(model)`, now over `ExecRelation::single_app` with `WitnessScope::All`).
  Their counters do not move (`pd` stays "stated 2" on those rows, §4.9). In a single-app view
  the cut graph and the cone graph are one assembler over the same calls (P13), so their
  output must be byte-identical. Step 9, which rebuilds them on the cross-app view, moves them
  onto the view's composition.
- **Roles, schema 4, the gap reachability filter**: #220 (step 8), merged first.
- **No batch, no `JobId` map** (step 12): one job per view, so the job's tree is the view's
  tree (6A-3).
- **No change to resolution or the L4 solve.** Histogram SHA equal; the L4 solve stays over
  `rel.primary()` (epic M5).
- **r3a3/r3a5 projections** (`project_r3a3`, `project_r3a5_cross_app`,
  `capability_cone.rs:4063-4131`) keep composing raw for every routine of their fixture
  universes, through `WitnessScope::All`.
- **Discoveries deferred** (each with its reason, §12): #215, #216, #217, #227, #228.

## 3. Background (at `459294f1`)

### 3.1 The compositions today

| Command (cross-app) | det cone (`detector_context.rs:1387-1394`, over `rel.cone()`) | snapshot cone (`capability_cone.rs:4582-4591`, over the cut) | counters `cone`/`pd`/`scc` (`tests/cli/cli_build_counters.rs`) |
|---|---|---|---|
| analyze default | `DerivedOnly`, every routine | none | 1/1/2 (`:192`) |
| analyze `--detector d47` | none (d47 demands no `SUMMARIES`) | `RawOnly`, every routine, from `fill_ordering` (`detector_context.rs:1114-1121` → `digest.rs:4016-4019`) | 1/1/2 (`:233`) |
| digest | `DerivedOnly` | `RawOnly`, after `det` is released (`digest_cli.rs:1632-1639`) | 2/2/3 (`:335`) |
| prove (single-app) | `DerivedOnly` | `RawOnly` (`prove.rs:890`) | 2/2/3 (`:389`) |
| policy check | `DerivedOnly` (the run) + `Both`, EVERY routine incl. dependency rows (`gate/policy/pipeline.rs:266-272`) | none | 2/2/3 (`:624`) |
| fingerprint (single-app) | `DerivedOnly` | `RawOnly` (`fingerprint_cli.rs:497`) | 2/2/3 (`:444`) |

Both compositions call `compose_inherited_cones` (`capability_cone.rs:2579`), whose
`routine_ids` is both the emission set and the raw-walk set (`:2819-2822`), and whose
`mode` gates the raw walk (`:2836`, recursive SCCs) and the singleton's raw half
(`inherited_facts_for_singleton`, `:2885-2898`). The predecessor fact cones
(`fact_cone_for_scc`, `:2713-2715`) and the coverage cones (`:2729-2742`) are built for every
SCC whatever the mode, so a per-member raw gate changes no other routine's output.

Two facts the pre-draft did not have:
- **policy check today composes raw for every routine on the cone graph**, dependency rows
  included (the `nodes` of `det_cone_inputs` are every model routine, `detector_context.rs:1266`).
  I5 is violated today by policy; step 6 fixes it.
- **analyze `--detector d47` builds no det cone** (`cone 1` is the snapshot's). So for `o`
  step 6 cannot fuse anything: the head must compose over the full cone graph where the parent
  composed over the cut (§Feasibility, A6.4).

### 3.2 What the tree's readers read

- digest S4 entries (`compute_digest_effects_cli`, `digest.rs:4094`): the root's own facts
  (`facts_by_routine.get(rid)`, `:3260`), every routine's DIRECT facts
  (`direct_facts_by_routine`, `direct_facts_by_op_kind`, `:243-315`), typed edges, the pd
  arena. Direct facts stay per routine of the view; only raw is scoped.
- the digest query (`run_digest_query_full_from_entries`, `digest_cli.rs:766`): `coverage`,
  `identities` and `unresolved_cone(snap, rid)` (`:990`; `unresolved_cone.rs:94`, `:118`:
  `callsite_index`, `callsite_resolutions`, typed edges).
- the ordering filter (`digest.rs:4035-4061`): `map_op` over every fact, so a root is kept on
  its own (direct + inherited) facts; the root is a subject.
- the ordering engine (`ordering_engine.rs:170-215`): `operation_index`, `callsite_index`,
  `callsite_resolutions` by caller, typed edges, and **`http_method_by_callsite_id` /
  `background_kickoff_callsite_ids` over EVERY fact, inherited included** (`:202-214`). This
  is the one reader that can look past the subject (Q3): an inherited fact of a non-subject M
  is keyed by M's call site. T3 decides it; F4 is the pre-stated fallback.
- root resolution (`resolve_changed_roots`, `digest_cli.rs:372`): `build_selector_indexes`
  (`selector_index.rs:84`) reads `snap.identities` and nothing else (P5).
- policy (`select_facts`, `policy_engine.rs:110-125`): `inherited_raw()` of primary routines
  only (`pipeline.rs:276-285`).

### 3.3 The pd arena

`derive_pd_arena` (`snapshot.rs:1701-1787`) **[#220]** maps every cone state into the tree:
`EdgeIx` into the tree's sorted `typed_edges`, `producer_fact` into its `capability_facts`
(`expect`ed, `:1745-1747`). A cone link whose edge has no tree row is dropped and counted
(`PD_EDGE_UNMAPPED`, `:1764-1769`; a `debug_assert!(false)` in debug). The tree is never
serialized with it (`PdArena` is in-memory, `snapshot.rs:914`).

## 4. Design

### 4.1 D1: the scope type (one walk, raw gated per member)

`ConeOutput` (`cone_derived.rs:244-263`) is replaced by:

```rust
pub enum WitnessScope { None, All, Subjects(BTreeSet<String>) }   // internal routine ids
pub struct ConeScope { pub derived: bool, pub raw: WitnessScope }
impl WitnessScope { fn contains(&self, id: &str) -> bool }
```

Keyed by the model's string id: 11b kept the cone string-keyed (`compose_inherited_cones`
takes `&BTreeSet<String>`, `capability_cone.rs:2585`; #215 is the per-row follow-up), so a
`RoutineIx` bitset has nothing to index (Decision 25 as 11b leaves it).

In `compose_inherited_cones` every member of `routine_ids` gets its coverage record and (when
`derived`) its derived row, as today; the member's raw half runs only when
`scope.raw.contains(m)`: the member computes `let m_mode = (derived, raw.contains(m))` and
passes it where `mode` is passed today (`:2836`, `:2876`, `:2885-2898`). A non-subject's
`inherited` is an empty `Vec`, as under `DerivedOnly`. Old modes map 1:1: `DerivedOnly` =
`{true, None}`, `RawOnly` = `{false, All}`, `Both` = `{true, All}`. The census counts subjects
and non-primary subjects (§4.10).

### 4.2 D2: the `caps` cell, one composition per view (G-1)

`View` gains `caps: Option<CapabilityAnalysis>` and `Cell::Caps`:

```rust
pub struct CapabilityAnalysis {
    pub direct_full: HashMap<String, Vec<CapabilityFact>>,   // det_cone_inputs, every routine
    pub cones: HashMap<String, ConeResultPub>,               // coverage every routine; inherited = subjects
    pub derived: ConeDerivedStore,                           // empty when !derived
    pub pd: Option<ConeArena>,                               // None after the effects cell
    pub subjects: WitnessScope,
}
```

`prepare` fills it right after `rel` with ONE `compose_cone_over_graph(rel.cone(), nodes,
&direct_full, &coverage_in, &ws.routines, ConeScope { derived: plan needs det summaries, raw:
plan.witness_scope() })`, the inputs from today's `det_cone_inputs(model, rel)`
(`detector_context.rs:1254-1293`, unchanged, so step 5's fold is the one fold). A plan that
needs neither det summaries nor witnesses (query, events without summaries) fills no `caps`.

`build_detector_artifacts` (`:1300`) stops composing: it takes the `caps` cell's `direct_full`,
`cones` and `derived` by move (the summary assembly at `:1408-1445` is unchanged except its
source) and passes `None` for the raw half of `FullRoutineSummary`, whose `Option` and
`inherited_raw()` are deleted (`full_summary.rs:54-130`). The resolved own entries
(`capability_cone.rs:2649`) stay a local of the one call: nothing is retained, nothing
re-solves (G-1). The Cone-SCC pair stays inside the composition (6A-2).

Rejected: (a) a separate witness composition after det (re-solve, "stated 2"): on `d` it
replaces a cut composition (0.65 s) with a full-graph one (≈ 6 s, §0 item 1) and puts
digest above its band; it is F5's fallback. (b) Retaining `resolved` across cells:
`ResolvedEntry<'g>` borrows the graph (`pd_solve.rs:91-105`).

### 4.3 D3: the subject set, fixed before the composition (Decisions 2, 14)

`Plan` gains `witness: WitnessScope`, set by the command's plan factory:

| Plan | Subjects |
|---|---|
| analyze without `ORDERING_FACTS`, events, query, fingerprint/diff/prove (their standalone composition is §2's) | none |
| analyze with `ORDERING_FACTS` (d47/d49/d51) | the internal ids of `reportable_roots(model)` (`digest.rs:3698-3727`: primary, body available, parse complete) |
| digest | the changed roots: `resolve_changed_roots` reads `identities` only (P5), so digest builds `derive_identity_table(model)` (`snapshot.rs:1372`, model only) before `prepare` and passes the roots in the plan; the `identities` table moves into the tree, not recomputed |
| policy check | every primary routine (`target.is_primary_routine`, `pipeline.rs:276-285`) |

Every subject is primary, so I5 holds by construction; the census asserts
`non_primary_subjects == 0` (T8). `resolve_changed_roots` takes `&SnapshotIdentityTable`
instead of `&CapabilitySnapshot` (P5: the one field it reads). A digest with no resolved root
keeps `WitnessScope::None` and still exits 2 (`digest_cli.rs:1682`).

Ordering subjects are every reportable root, not gated on ingredient bits (6A-6): today's
filter (`digest.rs:4035-4061`) keeps working on the tree's facts. F2 gates them if P1 says
the raw population is over budget.

### 4.4 D4: the tree, composed from `caps` and `rel` (replaces `R3a3SourceBase`)

`compose_snapshot_from_base(model, &R3a3SourceBase)` (`snapshot.rs:1250-1290`) **[#220]**
becomes `compose_tree(model, rel, &caps) -> CapabilitySnapshot`. `R3a3SourceBase` becomes a
struct of BORROWS with the same field roles, so each deriver keeps its body:

| Deriver (`snapshot.rs` **[#220]**) | Today reads | After |
|---|---|---|
| `derive_identity_table` (`:1372`) | model | the table digest built for its roots (moved in) or a fresh one |
| `derive_capability_facts` (`:1443-1486`) | `base.direct_full` + `base.cones[*].inherited` | `caps.direct_full` + `caps.cones[*].inherited` (empty for non-subjects) |
| `derive_typed_edges` (`:1501`) | `base.graph.typed_edges` (cut) | `rel.cone()` |
| `derive_operation_evidence`, `derive_callsite_evidence` (`:1842`, `:1968`) | `base.ws_routines` (a clone, `capability_cone.rs:4600`) | `&model.workspace.routines`, borrowed |
| `derive_callsite_resolutions` (`:2057`) | `base.calls` (cut, a clone `:4524`) | `model.calls.edges` then `rel.dependency_calls` (D6) |
| `derive_coverage` (`:2442`) | `base.cones[*].coverage` (the snapshot's own fold over the cut, `:4544`) | `caps.cones[*].coverage`: step 5's fold over `rel.uncertainty_all()` — the context's coverage (closes 5-A3) |
| `derive_event_declarations`, `derive_root_classifications`, `derive_routine_order_frames` | base | model (borrowed event graph, `model.events.graph`) |
| `derive_pd_arena` (`:1701`) | `base.pd_solution` | `caps.pd`, scoped (D5) |
| `derive_analysis_gaps` (`:2359`) | base | as #220 leaves it (its role rule and `rel.cone()` reach) |

Deleted: `build_r3a3_source_only_base`, `_over`, the cut's own clone of calls and event graph
(`capability_cone.rs:4523-4526`) and its fold (`:4542-4575`), the full-graph timings mode,
and the "legacy cut" doc text (`:4501-4504`, `combined_graph.rs:309`).

`compose_snapshot(model)` stays as the single-app convenience for prove, fingerprint, diff,
`snapshot_full` and tests (`digest.rs:3969`, `:3982`; `snapshot.rs:2713`): it builds
`ExecRelation::single_app(model)`, `det_cone_inputs`, one composition with `ConeScope {
derived: false, raw: All }` and `compose_tree`. Single-app this is today's snapshot (P13), so
its goldens must not move.

### 4.5 D5: the tree's pd arena, scoped (G-4, R6-4)

The tree's `PdArena` keeps today's shape (`EdgeIx` into the tree's sorted `typed_edges`,
`producer_fact` into `capability_facts`), so no digest reader changes (6A-5). It maps only the
states reachable from the subjects: start at `caps.pd.by_routine[s]` for each subject, follow
`links[..].callee_state`, keep each state once, in state order. `enumerate_proofs` starts at a
root's states and follows links (`digest.rs:880-905`), so a state outside that closure is never
read; T3 pins it. With `WitnessScope::All` the closure is every state: today's arena.

**R6-4, the link rule.** `derive_typed_edges` skips a row with no anchor; a cone link into a
skipped row is dropped and counted `PD_EDGE_UNMAPPED`, as in release today. The
`debug_assert!(false)` (`snapshot.rs:1767`) becomes a `debug_assert!` that the dropped edge is
of a kind `derive_typed_edges` skips. T6 hand-states one; the CDO gate asserts
`PD_EDGE_UNMAPPED == 0` on `d` and `o` (F6 if not).

### 4.6 D6: the ledger reads every caller (Decision 18, M8; Decision 8 ledger half)

`ExecRelation::new` (`exec_relation.rs:69-93`) **[#220]** keeps `dependency_calls:
Vec<CallEdge>` (the `dependency` argument, today dropped after assembly), read only by
`derive_callsite_resolutions` and printed as a `rel.dependency_calls` census bucket. The
ordering engine reads `callsite_resolutions` by caller (`ordering_engine.rs:187-192`), so on
the cone graph a walk through a dependency routine needs its rows: this half is load-bearing,
not cosmetic. In `classify_resolution` (`snapshot.rs:2267`) **[#220]**, a `"resolved"` call
whose every target is a body-less dependency row is `unfetched-dependency` with its edges
(today only `"opaque"` is, `:2339`). #220's T5 gains the ledger assertion (§11).

### 4.7 D7: cell order, effects and retention (G-2; epic §5.9 rule 3 as amended, 6A-1)

```
(1) program, graph job   (2) model, body released, rel
(3) caps: ONE composition (derived if det needs it; coverage every routine; raw = plan.witness; pd)
(4) tree from caps (only when plan.witness is not None)
(5) effects: digest S4 entries over the roots | the ordering cell over the tree;
    then caps.pd and the tree's pd and facts released
(6) det (moves direct_full, cones, derived out of caps; no cone), then l4, harvest; caps released
(7) runs; then the post-run projections (tx map, fanout, the policy rule engine: Decision 47)
(8) compute (the digest query)  → tree, model, rel, l4 released
```

What each plan keeps of the tree after (5):

| Plan | Kept through (6)-(7) | Freed at (5) |
|---|---|---|
| analyze with d47/d49/d51 | nothing (the ordering cell holds its facts) | the whole tree |
| digest | `identities`, `coverage`, `typed_edges`, `callsite_index`, `callsite_resolutions` (the query and `unresolved_cone` read them, §3.2) | `capability_facts`, `operation_index`, `pd_solution`, the rest |
| policy | no tree: the rule engine reads `caps`' raw for the primary subjects, kept beside `det` until (7) | — |

The digest tree's kept parts meet `det` and `l4`: that is new retention (today the cut
snapshot is composed after `det` is released, Decision 56). It is sized by the census at the
first run and bounded by B2. Fallback F3 computes `unresolved_cone` per root in the effects
cell and frees the whole tree at (5).

**Policy** reads `caps`' raw (`HashMap<String, Vec<CapabilityFact>>`, primary subjects) instead
of `inherited_raw()`; `select_facts` takes `(direct, raw)` slices. Deleted:
`pipeline.rs:255-272` (the RAW build), `substrate::RAW_INHERITED_FACTS` (`registry.rs:78`),
the assertion `session.rs:273-276`, and with them the tests that pin them (§5.9).

### 4.8 What is deleted

| Item | Where |
|---|---|
| `ConeOutput` | `cone_derived.rs:244-263`; call sites `capability_cone.rs:4118`, `:4590`, `detector_context.rs:1375-1379`, tests → `ConeScope` |
| det's own cone | `detector_context.rs:1386-1406` (the call; the summary assembly stays) |
| `RAW_INHERITED_FACTS`, `inherited_raw`, the `Option` | `registry.rs:32-90`, `full_summary.rs:42-130`, `capability_query.rs:62-102`, `session.rs:193`, `:273-276` |
| policy's RAW build | `gate/policy/pipeline.rs:255-272` |
| the cut graph | `capability_cone.rs:4476-4609`; `R3a3SourceBase` becomes the borrowed input struct |
| the full-graph timings mode | `capability_cone.rs:4611-4680`, `aldump.rs:61-70`, `:223` |
| ordering's own snapshot | `digest.rs:4016-4019` (`compute_digest_effects_for_ordering_with` takes the tree) |

### 4.9 Counters (I1; the final row)

New counter `build.witness_pass` = 1 per composition with a non-empty raw scope. Expected
(parenthesised = base, P9; ws_dep fixtures):

| Row | `bcg` | `cone` | `v2` | `scc` | `pd` | `ctx` | `snap` | `witness` |
|---|--:|--:|--:|--:|--:|--:|--:|--:|
| analyze cross-app default | 1 | 1 | 1 | 2 | 1 | 1 | 0 | 0 |
| analyze cross-app / single-app d47 | **1** (2) | 1 | 0 | 2 | 1 | 1 | 1 | 1 |
| digest cross-app / single-app | **1** (2) | **1** (2) | 1 | **2** (3) | **1** (2) | 1 | 1 | 1 |
| policy check cross-app / single-app | 1 | **1** (2) | 1 | **2** (3) | **1** (2) | **1** (2) | 0 | 1 |
| prove, fingerprint, diff | unchanged (§2: standalone composition, "stated 2") | | | | | | | 1 |
| query, events | unchanged | | | | | | | 0 |

For analyze-d47 the change is `bcg` 2 → 1: the composition reads the view's relation instead
of building the cut graph; its one cone and one solve now run over the cone graph. "1 per
view" holds on every row the step touches.

### 4.10 Memory and the census

| Item | Lives | Bucket |
|---|---|---|
| raw of the subjects | (3) → (5) (policy: → (7)) | `caps.raw` (`facts_tally`), plus the line `caps.raw subjects=N non_primary_subjects=0` |
| `caps.pd` (`ConeArena`) | (3) → (5) | `arena.link_graph` (exists) |
| the tree, scoped pd | (4) → (5) | `snapshot.pd_solution` (exists) |
| the tree, other parts | (4) → per §4.7 | `snapshot.*` (exist), printed at (4) and at the first run |
| `rel.dependency_calls` | the view | new `rel.dependency_calls` |

Base marks (`o`, cut): `snapshot.*` ≈ 128 MB shallow (facts 68.5 MB, pd 21.3 MB);
`arena.link_graph` 13.2 MB on the cut against 300.5 MB on the full graph (default analyze's
census). Peak expectation: the process peak stays in attach (3.15 GB on `d` and `o`). The risk
is the live set at (3)-(5) on `o`: the full arena (≈ 300 MB + 83 MB working), the raw of the
reportable roots (P1) and the cone-graph tree (P3). Policy's peak should DROP (14.3 GB today,
raw for every routine), reported in Part 3.

## 5. Test plan

Every new test lists its break (made with Edit, reverted with Edit), the failing assertion and
the revert; both outcomes are recorded (I9, the `discrimination-proof` skill).

- **T1 (#161, `mk.py` shape)** on `dep_dispatch_fixture` (`tests/common/dep_dispatch_fixture.rs`;
  `CallChain` → `Dep Lib::Outer` → `Inner`): cross-app `digest --routine CallChain` has DB_MODIFY
  on dependency Table 18 and COMMIT, each via `Outer → Inner`; its `coverage` equals the det's
  (`caps` coverage) for `CallChain`. Break: compose the tree over the primary half only (filter
  `rel.cone()` to primary callers, i.e. the cut) → both effects vanish → fails.
- **T2 (scope)**: root R calls M calls a writer W, subjects `{R}`: R's raw equals the
  `WitnessScope::All` run's; M and W have none; every routine has coverage and a derived row.
  Break 1: make the gate a no-op (`raw = All`) → "M has no raw" fails. Break 2: put the
  coverage record under the gate → W has no coverage → fails.
- **T3 (scoped-equals-unscoped oracle, Decision 14)**, test-only, per consumer, production
  scope vs `WitnessScope::All` on the same view, comparing the consumer's output BYTES: digest
  on every root of every cli-b `c.diff` and on the dependency fixtures; the ordering cell (the
  kept-root set and every `OrderingFacts`) on every r0 dir with the `Hyperlink` and rid-less
  write fixtures (Decision 38); policy's selected facts per primary routine (Decision 41,
  against today's `inherited_raw()` path on the parent's code, recorded as a fixture). CDO
  single-app (affordable: the single-app raw composition took 0.37 s at the base) as a
  CDO-gated arm. Discrimination (the whole-routine
  gate): make the raw walk skip a subject whose SCC is recursive → fails on a recursive r0
  fixture. Second break, the look-past reader (§3.2): drop non-subject facts from the ordering
  engine's call-site maps in the UNSCOPED arm too; a green run is recorded as evidence that the
  reader does not look past the subject on that corpus.
- **T4 (single-app identity, no-dependency parity, E9)**: every single-app golden unchanged;
  for every r0 dir, cross-app digest/analyze-d47/policy equal single-app except the stated
  header fields. Break: give the single-app `compose_snapshot` `Subjects(reportable roots)`
  → a non-root's inherited facts vanish from the fingerprint golden → fails.
- **T5 (one composition)**: `cli_build_counters.rs` rows restated to §4.9. Break: call the
  old det cone in addition → `cone 2, pd 2, scc 3` → fails.
- **T6 (R6-4)**: hand-stated edge in `rel.cone()` whose call site is absent from its caller's
  `call_sites`, carrying a pd link: no `typedEdges` row, the link dropped,
  `PD_EDGE_UNMAPPED == 1`, no panic in debug. Break: keep the row with a default anchor → the
  row-count assertion fails.
- **T7 (ledger)**: (a) a dependency caller's call is in `callsiteResolutions`; (b) a resolved
  call into a symbol-only dependency routine is `unfetched-dependency`. Break (a): leave
  `rel.dependency_calls` out → fails. Break (b): keep the `"resolved"` arm → fails.
- **T8 (I5, coverage equality)**: the census line reads `non_primary_subjects=0` on CDO `d`,
  `o` and policy; on the dependency fixture `tree.coverage[r] == det.summaries[r].coverage` for
  every routine. Break: add a dependency routine to policy's subjects → the census assertion
  fails.
- **T9 (`--diff` roots)**: on every cli-b diff fixture, roots resolved from the identities
  table equal roots resolved from today's composed snapshot. Break: build the table without
  the dependency rows → fails on the dependency fixture.

### 5.9 Tests deleted or rewritten with the code they pin (I2's exception)

Deleted: `session.rs:676-680` (`a det cell never carries RAW_INHERITED_FACTS`), the
`should_panic(expected = "RAW_INHERITED_FACTS")` tests (`capability_query.rs:295`,
`full_summary.rs:154` area), the `ConeOutput` parity tests rewritten on `ConeScope`
(`cone_derived.rs:1195-1780`, `capability_cone.rs` `Both` call sites), the
`detector_context.rs:2094` and `detector_context_step5_tests.rs:283` uses of
`build_r3a3_source_only_base` (rewritten on `compose_tree`; the step-5 test's claim
"context == snapshot base" becomes T8's), `detector_context_11b_tests.rs:1968`, the
`capability_cone.rs:6333`/`:6371` base tests, and any test of the timings mode. The plan names
each with its replacement.

## 6. Acceptance matrix

| # | Row 6 item | Evidence | Feasible on the measured data? |
|---|---|---|---|
| A6.1 | #161 acceptance | T1 | yes |
| A6.2 | context == snapshot coverage on the dependency fixture | T1, T8 | yes (by construction: one fold, one composition) |
| A6.3 | CDO digest wall and both peaks ≤ parent + band | B1, B2 (§8); post-attach peak not readable on Linux (6A-8, #204) | wall and process peak likely; post-attach peak partial (not measurable on Linux) |
| A6.4 | same for analyze with d47/d49/d51 | B3, B4 | **wall: not deliverable as worded** (≥ +5 s by construction, §0 item 1); delivered as 6A-10's cost bound. Peak: unknown until P1 (S2) |
| A6.5 | counters at the final row | T5, §4.9 | yes |
| A6.6 | no raw `Vec` for any dependency subject | T8; CDO census line | yes (by construction) |
| A6.7 | CDO `policy check` moves nothing | Part 1 `cmp`, custom and bundled policy, cross-app | yes (same composition, same facts for primary routines; P10 records the parent) |
| A6.8 | stop rule | §8.4, read after Task 1, before Task 2 | yes |
| A6.9 | `mk.py` test, revert to cut → fails | T1's break | yes |
| A6.10 | scope discrimination; scoped-equals-unscoped oracle with the whole-routine-gate discrimination | T2, T3 | yes; the CDO cross-app arm runs only as §8 Part 1's ordering/digest diff (unscoped cross-app CDO does not fit, Q6) |
| A6.11 | ordering root-set oracle (`Hyperlink`, rid-less) | T3 | yes |
| A6.12 | policy facts oracle | T3 | yes |
| A6.13 | `--diff` root resolution byte-equal | T9 + T3's digest arm | yes |
| A6.14 | no-dependency parity (E9) | T4 | yes |
| A6.15 | `RAW_INHERITED_FACTS`, `ConeOutput::Both`, `inherited_raw`, `LegacyCut`, `build_r3a3_source_only_base` deleted | `rg` empty in `src/` | yes |
| A6.16 | `unfetched-dependency` ledger rule | T7 | yes |
| A6.17 | R6-4 serialization filter and link rule | §4.5, T6, CDO `PD_EDGE_UNMAPPED == 0` | yes, unless F6 |
| A6.18 | `pd` arena in view identity, one per view, never alive during a run (E18) | §4.7 order; census at the first run shows no `arena.*` / `snapshot.pd_solution` | yes |
| A6.19 | goldens: cross-app digest in `cli_digest_query_dependencies`; `r4_cross_app_program` if it covers d47/d49/d51 | moved, each move triaged (`golden-diff-triager`) | yes |
| A6.20 | CDO deltas triaged | `cdo-measure` on `o` and `d`; > 30 % FP on new findings → root-cause fix (the three are already opt-in) | yes |
| E14 | CDO ratchets | `bash scripts/cdo-gate` | yes |

## 7. False-positive and failure shapes

- **New d47/d49/d51 findings** (the cone graph sees dependency writes and IO): a finding
  through a dependency-internal path is the point of #161; one whose dependency path is not
  executable from the root is a root-cause fix (F8).
- **Changed digest coverage on CDO** (`complete` → `partial` for roots whose only four-kind
  edge is in dependency code; step 5 counted 965 primary routines flipping on the dependency
  half alone): the correct answer since step 5 (5-A3); reported, not gated.
- **A reader looks past the subject** (§3.2): T3 fails closed; F4.
- **The scoped pd closure misses a state a proof needs**: T3's digest arm.
- **A dependency subject gets raw**: T8.
- **An anchorless edge carries a link**: T6; CDO must be 0 (F6).
- **The digest tree meets `det`** (§4.7): census + B2; F3.
- **A vacuous no-regression bound**: §8.1's band-width void check.
- **Noise**: server1 is shared; the load gate and the one re-run of 10+11a are reused.

## 8. Measurement plan

One CDO bench, run ONCE after the final panel, through `agentflow run --name bench --timeout
60 -- <cmd>`, in 10+11a's three-part shape (`/work/bench/160-6/bench-draft.sh`, updated for
6A-10). `WS` from `.agent/config.json` `cdo_ws`; `P` = `459294f1` (binaries already built,
`/work/bench/160-6/parent/bins.sha`; re-based onto the post-#220 master if the branch is
rebased, then rebuilt), `H` = the final-panel head; release-fast from clean detached
worktrees; `RAYON_NUM_THREADS` unset; peaks by `maxrss.py` (kernel `VmHWM`).

**Quiet slot.** The load gate (1-min load ≤ 4, wait ≤ 15 min, then fail closed) runs before
Part 2 AND the load ticks of Part 2 are read before the bounds: a tick above 8 during Part 2
counts as a void run (the one re-run follows). The conductor schedules Part 2 when the other
session runs no bench or build.

- **Part 1 (identity and moves).** Histogram SHA equal (`c032c8db…`). SARIF `cmp`: cross-app
  default analyze (`--scope primary`, `--scope all`) and `--single-app` analyze (both scopes)
  identical. Single-app `analyze --detector d47,d49,d51` identical (cut = cone single-app).
  `policy check` cross-app, custom and bundled policy: `cmp` against the parent (A6.7; parent
  SHAs `351d6a57…`, `938fa67a…`). Single-app `fingerprint --format json`: `cmp`. DIFFED, not
  compared, every move triaged (A6.20): cross-app `o` SARIF (parent: d47 2, d49 6) and cross-app
  `d` JSON. `ALSEM_RETAINED_CENSUS=1` per side: §4.10's buckets and the I5 line.
- **Part 2 (bounded).** `bench_ab.py run --rounds 5`, interleaved, `d` and `o`. Then 3 rounds
  per side and command through `maxrss.py`.
- **Part 3 (reported, no bound; Decision 58).** 3 rounds: cross-app `policy check` (wall and
  peak: expected to DROP), cross-app default `analyze`.

### 8.1 Bounds (bands from this bench's parent runs; I6, Decision 20)

| Bound | Reading | Rule | Void check (read first) |
|---|---|---|---|
| B1 (A6.3 wall) | `d`'s process wall | `Hd ≤ Pd + Bd` | `Bd ≤ 0.05 × Pd`, and no Part 2 load tick > 8 |
| B2 (A6.3 peak) | `d`'s kernel peak | `Hm ≤ 1.03 × Pm` | none |
| B3 (A6.4 wall, 6A-10) | `o`'s `analyze.total` | `Ho ≤ Po + C + R + E + Bo`, C/R/E fixed in the ledger before Task 2 (§8.4) | `Bo ≤ 0.05 × Po`, load as B1 |
| B4 (A6.4 peak) | `o`'s kernel peak | `Hm ≤ 1.03 × Pm` | none |
| — post-attach peaks | not readable on Linux (#204) | reported "not measured" (6A-8) | — |

A void check → re-run Part 2 once (fresh 5 rounds), read every bound from the re-run alone;
void again → the bound FAILS (F9). Missed (met-not-void) → F10.

### 8.2 Expected (not a claim)

`d`: − the cut base (≈ 0.95 s at the base) − the cut derive (0.99 s) + the cone-graph derive
(P3 head) + the raw walk of 3 roots. `o`: + C (≈ 5.5 s: the full composition 6.1 s minus the
cut's 0.68 s) + R + E.

### 8.3 Numbers file

`docs/measurements/2026-10-<dd>-issue-160-step6.md`, 10+11a's sections, plus the Task 1 probe
numbers and the stop rule's reading.

### 8.4 The stop rule (Decision 52), read after Task 1, before Task 2

Task 1 lands the scope type and the `caps` cell behind the old wiring (nothing switched) plus
a measurement-only `aldump --issue160-step6-probe <ws>` (deleted in Task 2's commit). It
composes once over `rel.cone()` with each subject set (none, the 3 roots of `d`, the reportable
roots, every primary routine) and prints: subjects, raw facts and bytes (`facts_tally`), raw
walk ms, `pd_solve` ms, scoped pd states/links/bytes, `PD_EDGE_UNMAPPED`, anchorless rows with
links, the tree derive ms and bytes per part, the ordering kept roots and the ordering effects
ms on the cone graph, `rel.dependency_calls` count and bytes, `non_primary_subjects`. These are
P1, P2, P3, P4, P8; P6's dynamic half is T3 on the corpus plus a CDO single-app run. Three
rounds each, in the bench slot. The ledger records, before Task 2: **C** = the full
composition minus the cut composition (parent `context.capability_cones` of default analyze
minus `ordering.compose_snapshot`'s compose half), **R** = the reportable roots' raw walk ms,
**E** = the ordering effects ms on the cone graph minus the parent's `ordering.digest_query`.

- **S1 (wall).** Projected `d` = `Pd − 0.95 s − cut derive + P3(roots) + R(roots)` ≤ `Pd +
  Bd`. Projected `o` cost `C + R + E` ≤ **0.5 × Po** (≈ 8.7 s; the cap of 6A-10).
- **S2 (memory).** On `o`: R bytes + scoped pd bytes + tree bytes + `arena.link_graph` + arena
  working ≤ 1.0 GB (the attach peak minus the post-attach live set, epic §5.12, less 3 %).
  On `d`: the same with the 3 roots.
- S1 or S2 fails → F1, then F2, then F11 (and F3 for `d`), re-projecting after each. Still
  failing → the claim stops at Task 1 (scope type and cell merged behind the old wiring, nothing
  switched), files "step 6: cone-graph witnesses exceed budget on CDO" with the numbers, and the
  epic re-plans. No bound is relabelled.

## 9. Amendments this spec asks the epic to take

- **6A-1 the witness pass is the raw half of the ONE composition**, not a second `RawOnly`
  call; §5.7's "`compose_cone_over_graph(.., RawOnly)` stays the witness engine" and §5.9 rule
  3 items (3)-(5) become §4.7's (3)-(5). G-1: neither retain nor re-solve. The det builder
  loses its cone; Decision 15 ("pass before `det`") holds.
- **6A-2 the Cone-SCC pair stays in the composition** (M3 withdrawn): one composition per view
  leaves no second reader.
- **6A-3 one job per view until step 12**: the job's tree is the view's tree; the
  `BTreeMap<JobId, SnapshotTree>` arrives with step 12.
- **6A-4 Decision 8's gap half is #220's** (step 8, merged first; its T5); step 6 does the
  ledger half.
- **6A-5 the arena's `EdgeIx` indexes the tree's sorted `typed_edges`**, not `rel.typed`, and
  the arena is scoped to the subjects' state closure: no reader indexes `rel.typed`.
- **6A-6 ordering subjects are the reportable roots, not membership-gated**, unless F2 fires;
  Decision 23/38's three bits land with F2.
- **6A-7 step 6's wall bounds are no-regression bounds with a band-width void check**
  (`B ≤ 5 % of P`), since I6's "≥ 2 × band" rule is written for savings.
- **6A-8 10A-4 carried**: post-attach peaks are reported "not measured" until #204.
- **6A-9 prove, fingerprint and diff stay on a standalone composition until step 9** (§2):
  they are single-app at step 6, their composition is today's bytes, and step 9 rebuilds them
  on the cross-app view anyway. The row's "1 per view" holds for the commands step 6 moves;
  those three keep "stated 2". The scoped-equals-unscoped oracle's prove/fingerprint arms are
  trivially met (their scope is `All`) and move to step 9.
- **6A-10 analyze with d47/d49/d51 carries a pre-stated COST bound, not "≤ parent + band".**
  The parent composes over the cut; the head must compose over the cone graph, which costs one
  full composition more (§0 item 1). The bound is `Ho ≤ Po + C + R + E + Bo` with C, R, E
  measured at Task 1 and recorded before Task 2, and the stop rule caps `C + R + E` at
  `0.5 × Po`. Reason: the row's bound cannot be met by any design that delivers #161 for the
  ordering detectors; keeping the ordering cell on the cut would keep the cut graph the row
  deletes. The peak bound (B4) stays as worded.

## 10. Fallbacks (every "if the measurement shows X")

Every fallback that changes code re-runs every identity gate it could affect (T3, T4, the
golden targets, §8 Part 1) before the claim continues. None needs a new panel round; each is
recorded in the ledger.

| # | If | Then |
|---|---|---|
| F1 | S2 fails on raw (P1 bytes of the reportable roots) | release each root's raw as the ordering cell consumes it (the cell reduces one root at a time, `digest.rs:4011`), so peak raw ≈ one worker's roots; re-project |
| F2 | S1 or S2 still fails on `o` | gate the raw walk by the three ordering bits on the derived row (Decision 38, `map_op`'s vocabulary), so raw exists only for roots with an ordering ingredient; T3's ordering arm proves the kept-root set unchanged; re-project |
| F3 | B2 or the census shows the digest tree's kept parts too large | compute `unresolved_cone` per root in the effects cell and free the whole tree at (5) |
| F4 | T3 fails on a reader that looks past the subject | widen THAT reader's input to the subjects' callee closure (from `rel.cone()`), per reader; never give a dependency routine raw (I5): if a dependency routine is in the closure, the reader reads its direct facts and derived row instead |
| F5 | D1 cannot be made byte-identical on derived rows (Part 1 default-analyze SARIF differs) | a defect; not closed in one attempt → a SEPARATE witness composition (re-solve, "stated 2"); re-run the stop rule with its cost |
| F6 | `PD_EDGE_UNMAPPED > 0` on CDO, or a dropped link carries a proof | triage the edge kind; fix the anchor source; not closed → stop at Task 1 |
| F7 | CDO `policy check` differs | a defect; not closed in one attempt → stop, nothing merges |
| F8 | new d47/d49/d51 findings > 30 % FP on the triage sample | root-cause fix first (the detectors are already opt-in) |
| F9 | B1 or B3 void twice | the bound FAILS: claim blocked |
| F10 | B1-B4 met-not-void but missed | name the miss (`trace_summary.py` span, census bucket); ONE fix from F1-F3 or F11; re-run Part 2 once; still missed → FAILS |
| F11 | S1 short by the cross-app `direct=` time | #228: the cross-app `caps` fill reuses `build_cross_app_base`'s `direct_full`/`direct_coverage` instead of `det_cone_inputs` (step 5's T5 already pins the maps equal); re-project |

## 11. What #220 hands this step (re-cite after it merges)

- Every **[#220]** citation (snapshot.rs derivers, `classify_resolution`, `derive_pd_arena`,
  `exec_relation.rs`, `selector_index.rs`, `digest.rs` near roots, `registry.rs`).
- (a) `compose_full_snapshot` keeps `opts.universe` (`SnapshotUniverse`) and the class filter;
  the scoped tree must produce exactly the rows that filter keeps (#220's T9/T10 are the
  oracle). (b) #220's T5 gains the `unfetched-dependency` ledger assertion. (c) #220's T9d
  gains "`Inner`'s operation is referenced by `CallChain`'s inherited fact, so it is kept".
  (d) `derive_analysis_gaps` keeps calling #220's role rule when its input moves to `caps`.
- #220's `derive_analysis_gaps(resolved, base)` signature and its `rel.cone()` reach replace
  the stub `is_dependency_role` (`snapshot.rs:2433`): D4's table row reads "as #220 leaves it".

## 12. Discoveries: in scope or deferred

| Issue | Verdict | Reason |
|---|---|---|
| #215 per-row cone and solve | deferred | changes per-id semantics (a collided id's derived row); needs its own triage; row 6 does not need it (the scope is keyed per id like the cone) |
| #216 guard frames of `#if` arms | deferred | rides on #215's per-row cone |
| #217 twin-node fan-out | deferred | D6 adds rows for dependency callers but does not change how an edge picks a row; routing an edge to every row of a node moves results and needs its own triage |
| #227 dependency L4 solve | deferred | row 6 changes no L4 solve (non-goal); the dependency solve moves d1/d2/confidence readers; best after step 9 |
| #228 one cross-app producer | deferred as fallback F11 | not needed for correctness (step 5's T5 pins the two maps equal); it is the first rung if S1 is short by the `direct=` time |

## 13. Open questions (recommended answers)

1. **Q1. 6A-10: accept a stated cost for `o`?** *Recommended: yes*, with the cap
   `C + R + E ≤ 0.5 × Po` read before Task 2. The alternative, keeping the ordering cell on
   the cut, keeps the code the row deletes and leaves #161 open for d47/d49/d51.
2. **Q2. 6A-1: fused composition, not a second `RawOnly` pass?** *Recommended: yes.* A second
   full-graph pass costs ≈ 6 s on `d` and puts B1 out of reach; F5 keeps it as the fallback.
3. **Q3. Does a reader look past the subject?** The ordering engine's call-site maps
   (`ordering_engine.rs:202-214`) can by construction. *Recommended:* T3 decides on the corpus
   and on CDO single-app; F4 is the pre-stated fix. Not decidable before code.
4. **Q4. Policy timing (Decision 47).** The rule engine runs off `det` at (7), so primary raw
   lives through `det` and the run. *Recommended: keep Decision 47*; primary raw is a fraction
   of today's every-routine raw, so policy's peak drops either way. Revisit only if Part 3
   shows the peak does not drop.
5. **Q5. 6A-9: prove, fingerprint, diff on a standalone composition until step 9?**
   *Recommended: yes* (smaller step; their output is single-app and byte-identical).
6. **Q6. The scoped-equals-unscoped oracle on CDO cross-app.** Unscoped cross-app CDO does not
   fit (step 0: 31.4 GB). *Recommended:* the CDO arm is single-app (cheap: the single-app raw
   composition is 0.37 s) plus Part 1's triaged diff of `d` and `o` cross-app; the corpus and
   the dependency fixtures carry the cross-app oracle.
7. **Q7. Void-check noise.** *Recommended:* keep `B ≤ 5 %`, run Part 2 in a quiet slot with the
   load-tick rule (§8); a twice-void bound fails (F9) and blocks the merge rather than being
   relabelled.
8. **Q8. `build.snapshot` for the tree at (4)?** *Recommended: yes*, one per tree composition.
9. **Q9. Ordering subjects gated by membership from the start (Decision 23)?** *Recommended:
   no* (6A-6): F2 adds the gate only if P1 says the raw is over budget.
