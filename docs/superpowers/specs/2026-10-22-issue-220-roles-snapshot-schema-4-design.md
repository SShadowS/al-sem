# #220 (epic #160 step 8): roles, snapshot schema 4, preflight

Status: revised after spec panel round 1 (2026-10-22; findings S1-S17 from slot astra/sol and
O1-O18 from slot flash/opus, register `.agent/issue-220/findings.json`). Made from the
pre-draft `/work/bench/160-8/draft-step8-design.md` (written at `5c42b8f3`, unmeasured) once
the probes had run. §0 lists what the probes changed; §0.1 what round 1 changed.

Child of `2026-10-18-issue-160-analysis-session-design.md` (the epic): §6 "Roles, one rule",
"Gaps", "Snapshot schema 4"; §9 row **8**; E6, E7, E8, E14; I2, I4, I8, I9, I10; Decisions 8,
13, 17, 28, 46, 58. Shape follows `2026-10-22-issue-160-step11b-routine-rekeying-design.md`
and `2026-10-22-issue-160-steps10-11a-siteix-routine-identity-design.md`. Where this document
and the epic disagree, this document is wrong, EXCEPT for the numbered amendments of §9 (8A-1
to 8A-6).

**Base:** master `f96619ec` (step 11b merged). Steps 5 and 6 are NOT merged. The operator split
step 8 out (issue #220) so it merges BEFORE step 6's implementation starts; step 5 runs in
parallel in another session. Every `file:line` below is in `f96619ec`. Every fact the design
rests on is a row of the ledger's `## Assumption probes` table (`.agent/issue-220/ledger.md`,
P1-P12, P1c). Probe output, the probe patch (reverted) and the CBOR decoder are in
`/work/bench/160-8/`.

Words:
- **Role** = `Primary` or `Dependency`: whether a row (routine or object) belongs to the
  workspace's own app (`Model::primary_app`) or to a dependency app.
- **Universe** = which model a snapshot describes: `single-app` (the workspace only) or
  `cross-app` (the workspace plus the dependency code it demands).
- **Cone** = `ExecRelation::cone()` (`exec_relation.rs:112`), the typed edges of every caller,
  dependency callers included. **Cut** = the snapshot's own graph today: the primary callers'
  calls plus event dispatch (`capability_cone.rs:4564-4567`, "the LEGACY CUT").
- **Reachable** = reachable from some primary routine over the cone.
- **Gap** = a row of the snapshot's `analysisGaps`.
- **Key class** = the rule that decides which rows of one top-level snapshot key are
  serialized (Decision 13).
- **Reference field** = a field of a serialized row whose value is an id of another row, in one
  DOMAIN (§4.4, `REFERENCES`). **Referenced** = a row a reference field's value resolves to.
- **Preflight** = the checks `alsem diff` runs before comparing (`gate/diff/preflight.rs`).

## 0. What the probes changed in the pre-draft

1. **F9 fires (P6).** The snapshot is still built on the cut (step 6 has not merged). On CDO,
   22,937 dependency rows are reachable over the cone but only 361 over the cut. Reach must be
   computed from `rel.cone()`, which the caller passes in (D2). The pre-draft took it from step
   6's snapshot graph.
2. **8A-5 is withdrawn (P5).** Body-less dependency rows are exactly the ABI rows of symbol-only
   apps: the cross-app model adds EVERY ABI row of a required symbol-only app and only parsed
   rows of an app with source (`workspace.rs:1600-1634`; interface and ControlAddIn methods are
   never routines, `:989-993`; `apps_mixed_body` = 0 on CDO and on 4 fixtures). So today's
   grouping ("an app owning a body-less dependency row") already is "the opaque app".
   `body-unavailable` stays dead, and Decision 8's gap is the app-level `symbol-only-boundary`
   row. The cross-app serialization asserts it (D2, O16).
3. **F7 fires.** Today a call with resolution `opaque` is already ledgered
   `unfetched-dependency` (`snapshot.rs:2339`); what is missing is step 6's rule that a call
   RESOLVED into a body-less dependency row is ledgered so too. T5 asserts the gap alone; step 6
   adds the ledger half (§12).
4. **P1 found a third role rule:** 11b's `projection::role_of` (`program/model/projection.rs:97-111`)
   reads a row's `source_unit_id` (`ws:` = primary). It agrees with the app-guid rule on every
   CDO and fixture row (P1c). It is kept (§2). Round 1 found a fourth (O1, §3.1: d1's map).
5. **P3, letter falsified, design unchanged.** A single-app CDO snapshot names 5 dependency apps
   outside `inputs`, but only in `eventDeclarations[].eventId` and `.binding.publisherObject`
   (primary subscribers of dependency events). `identities` holds the primary app only, so
   single-app `apps[]` stays the primary row.
6. **CDO has nothing to gap (P5).** Every CDO dependency has source: 0 body-less rows, 0
   parse-incomplete rows, `opaque_apps` 0. The gap filter and the gap half of the key classes
   have no CDO witness. `tests/r2-5b-fixtures/cross-app-resolution` is the witness (2
   symbol-only apps, 8 body-less rows, only app `dddd…0001` reachable).
7. **The other assumptions hold:** the stable-id prefix, the object-id guid, `dep_routine_ids`
   and the registry map all agree with the app-guid rule on CDO (P1b: 31,658 rows and 11,133
   objects, 0 disagreements; P8: 42,625 keys, 0). 11b's role-homogeneity assertion holds on CDO
   (P7). The allow-list's decoder reads all 86 cli-b CBOR files equal to their raw JSON (P4b).
8. **New discovery (P9):** a FATAL diff preflight exits 0, even under `--fail-on`
   (`gate/diff/mod.rs:145-153`, `diff/cli.rs:299-307`). Taken in scope (D5.5).

### 0.1 What spec panel round 1 changed

- **Reference closure is typed, not a string scan** (S1, O3, S9, O17). Internal table ids
  (`<guid>/table/N`) map to `<guid>:Table:N`, not to a `/`→`:` rewrite; only listed reference
  fields are read, so a literal that looks like an id is never kept. D4 states the order of the
  filter steps.
- **Filters run on typed rows, before CBOR** (O7). Dependency rows never become `CborValue`s;
  this step's filter is the oracle step 6/9's scoped composition must match.
- **Evidence keeps primary rows plus referenced rows**, and `routineOrderFrames` gains the
  frames of every kept evidence row's routine (S2). A cross-app snapshot of a no-dependency
  workspace therefore equals the single-app one.
- **Shards keep reference closure** (S3) and match app guids case-insensitively (O13).
  `slice_for_app` keeps its own membership fields (O4).
- **Load-time validation of roles** (O6, S4, S14): exactly one `primary` row, every role known;
  a dependency shard is refused as a diff input.
- **`routineInventory` is classed** and `analysisUniverse` sits in the inventory payload (S5, O8).
- **Tests fixed so every break can fail** (S6, O2, S7, O5, S8, S13, S15, O15): a cone-only arm,
  a composer-order arm, Evidence-row arms, a parity arm with a hand-stated gap.
- **d1's all-primary anchor map is listed and kept** (O1), and filed as a discovery.
- Fallbacks F17/F18 rewritten (S9, S10); new F19-F23 for the new "if X"s.
- The allow-list checks exact values, key order and the PROVENANCE edit (S11, S12, O9) and is
  step 8's check only (S16). Wording narrowed (S17, O12); grep widened (O10); compile-affected
  tests named (O11); `apps[]` for an unknown app (O14); `rel` from another model (O18).

### 0.2 What spec panel round 2 changed

- **References are typed by domain** (SN1, SN2, SN3, ON1, ON3, ON4). §4.4's `REFERENCES`
  table names, per key, the field that DEFINES the row and each field that REFERS to another
  row, with its domain: identity, app, operation, callsite, frame, edge. Each domain has one
  destination and one matching rule. Operation and callsite ids match Evidence rows verbatim;
  a row's own id is never a reference to itself; `permissionFacts.target` is listed; a stable
  event id gives its object by its first three `:` segments.
- **Closure is checked per domain** against the unfiltered rows, and a model-rooted value
  that resolves to nothing is a violation. So a wrong table mapping fails on its own (SN4).
- **Shards are closed over more than identities** (S3): evidence rows and identities, to a
  fixpoint. (Round 3 narrowed it: the frame and app domains are not closed in shards, §0.3.)
- **The model is bound to the universe by pointer** (O18): a cross-app universe carries its
  base, and composing it with any other model panics (T12).
- The A8.1 grep lists its expected non-role hits (ON2). New tests T9g and T12; new fallbacks
  F24-F28.

### 0.3 What spec panel round 3 changed

- **Shards do not close the frame or app domains** (S3N1, O3N1). Shards have never carried
  `routineOrderFrames` (`slice_for_app` skips it, `snapshot_full.rs:1326-1333`; the three
  committed `primary.json` shard goldens have none, while the full tree has one), and they
  copy `analysisGaps` whole while keeping one `apps[]` row. Closing those domains would change
  the shard shape and move existing goldens, so no shard golden may move beyond the
  allow-list's header and `role` changes (§8 Part 0).
- **Nested references are listed** (S3N2): `derivedFromCapability.witnessFirstHopOperationId`,
  and every value-source field (`resourceArgSource`, `extra.bodyArgSource`,
  `extra.keyArgSource`, `extra.valueArgSource`, `targetIdSource`), read recursively through
  `initializer`.
- **"External" is an explicit list** (S3N3): only identity values of listed shapes may stay
  unresolved; operation, callsite, edge, frame and app references must resolve, except the
  root frame's `parentFrameId = -1`. T9-closure breaks each domain once.
- **T10 gets a real unreferenced table** (SN2): a temp copy of its fixture with one unused
  table. F17 names `REFERENCES` (O3N2). New fallbacks F29-F30; F25 and F26 rewritten.

## Feasibility first

| Item | Verdict | Why (tree at `f96619ec`) |
|---|---|---|
| Every cli-b diff checked against an allow-list | **feasible, narrower than it reads** | Only `fingerprint` and `diff` compose the full snapshot (`fingerprint_cli.rs:436`, `gate/diff/cli.rs:140`); both are single-app until step 9. In a single-app model every row is primary (P3), so every key class keeps every row: only the header and `apps[].role` move. Every cli-b `analysisGaps` is `[]` (P4), so the gap annotation moves no cli-b byte |
| Live `is_dependency_role` with reachability | **feasible; no command answer sees it at step 8** | Stub `snapshot.rs:2433-2435`. Gaps are serialized only by the full snapshot (single-app) and counted by `prove` (single-app, `prove.rs:882`, `:976-984`). `digest` composes a cross-app in-memory snapshot (`digest_cli.rs:1639`) but never reads gaps. Library tests on a cross-app composition carry the proof until step 9 |
| Reachable-only gap test | **feasible, with a committed fixture** | `tests/r2-5b-fixtures/cross-app-resolution`: app `eeee…0002`'s rows are model rows (all ABI rows of a required symbol-only app are added) and none is reachable (P6) |
| Mixed-universe refusal | **feasible** | one more fatal diagnostic beside `app-identity-mismatch` (`preflight.rs:83-101`); plus the exit-code fix (D5.5) so the refusal is visible to CI |
| Shards with a dependency `apps[0]` | **feasible; latent defect today** | `serialize_sharded` takes the primary from `apps[0]` (`snapshot_full.rs:1280`); preflight too (`preflight.rs:25-30`); `project_apps` sorts by guid (`:398`). Position = role only while `apps[]` has one row |
| One test per key class, gaps in both directions | **feasible on a library cross-app composition** | `AnalysisTarget::build(inputs, false, ..)` (`gate/run.rs:179-200`, over the `pub(crate)` `build_cross_app_base`, `capability_cone.rs:3856`) and `compose_full_snapshot` are `pub`; the `dep_dispatch_fixture` (embedded source) and the r2-5b fixture (symbol-only) give every shape |
| CDO identity (Decision 58) | **feasible: the rule is today's rule** | P1b, P1c, P8: 0 disagreements; d1's map is not converted (O1) |

## 1. Goal

1. **One role rule.** `Role` is one function of a row's `app_guid` against
   `Model::primary_app`, for routines and objects. It replaces the snapshot stub, the
   registry's two role maps, the base's id set and the other copies of the comparison (§3.1).
   The `dep:` location filter (`registry.rs:544-558`) stays beside it, unchanged. Two rules stay
   on purpose, each stated in §3.1: 11b's `role_of` and d1's all-primary anchor map.
2. **Live gaps, filtered by reachability when serialized.** `is_dependency_role` becomes
   `role == Dependency`. The in-memory gap table is unfiltered (I8) and every gap row carries
   its `role`. The serialized `analysisGaps` of a cross-app snapshot keeps a dependency gap only
   when its subject is reachable (Decision 46).
3. **Snapshot schema 4.** `schemaVersion: 4`; a header key `analysisUniverse`
   (`"single-app"` | `"cross-app"`); `apps[]` rows carry `role`, primary first; gap rows carry
   `role`. Every top-level key has a class in code; a cross-app serialization applies it, and
   every serialized reference resolves inside the snapshot.
4. **Preflight.** Schema 3 is refused at load with a remedy. A v4 file must name its universe
   and exactly one primary app. Two snapshots of different universes are refused. App identity
   and shard roles read `role`, never `apps[0]`. A fatal preflight exits 1.

Why now: step 9 makes `fingerprint`, `prove` and `diff` cross-app. Without step 8 a cross-app
snapshot would carry every demanded dependency row (demand churn reads as a behaviour diff),
never report a symbol-only boundary (prove could answer PROVEN through an opaque app, E6), and
diff silently against a single-app snapshot of the same workspace (E8).

## 2. Non-goals

- **No command changes universe.** `fingerprint`, `prove`, `diff` stay single-app; their
  production callers pass `SnapshotUniverse::SingleApp`. `--single-app` on them, the cross-app
  `View` for them and `"cross-app"` in cli-b are step 9's. Decision 28 (the fingerprint query
  path filters by role) is step 9's; step 8 supplies `Role`.
- **No change to resolution, demand, the cone or coverage.** Histogram SHA equal. Reach is a
  serialization filter, not a smaller model.
- **The `dep:` location filter is not folded into `Role`** (epic §6).
- **`dep_routine_ids` is not re-keyed.** Its 71 reads in 14 files (P2) keep the string-keyed
  set; the set is BUILT from the rule (8A-2).
- **11b's `projection::role_of` is not replaced.** It runs inside `RowsOfId::build(routines)`,
  which has no `Model` (no primary guid); threading one through its callers is churn for a rule
  that agrees everywhere (P1c). Its USE (the homogeneity check) is pinned by 11b's own test
  through the production path (`detector_context_11b_tests.rs:1927-1940`); T1c here is a
  supplementary agreement check, not use coverage (S13).
- **d1's anchor map is not converted** (O1, §3.1).
- **`body-unavailable` is not revived** (§0.2).
- **No digest or prove output change**, and no change to the uncertainty split's halves (step
  5's): step 8 swaps only the predicate that builds `dep_routine_ids`.
- **No bench** (Decision 58).

## 3. Background

### 3.1 Where roles are decided today (P1, plus O1)

| Site | Rule today | After step 8 |
|---|---|---|
| `capability_cone.rs:3866-3880` (`build_cross_app_base`) | `app_guid.to_ascii_lowercase() != primary` → `dep_routine_ids` | built from `routine_role` |
| `capability_cone.rs:3904-3906` | call split on `dep_routine_ids.contains(&e.from)` | unchanged (reads the projection) |
| `capability_cone.rs:3743, 4000, 4035` | `primary_app_guid` field (lowercase) + its census bucket | deleted (its one reader, the registry, calls the rule) |
| `registry.rs:449-456` (single-app) | every routine `"primary"` | `routine_role(r).as_str()` |
| `registry.rs:524-543` (`cross_app_scope`) | routines via `dep_routine_ids`; objects by lowercase compare (`:536`) | `routine_role` / `object_role` |
| `gate/run.rs:244-249` (`is_primary_routine`) | `!dep_routine_ids.contains(&r.id)` | `routine_role(r) == Primary` |
| `gate/run.rs:414-421` (`dependency_object_predicate`) | `!o.app_guid.eq_ignore_ascii_case(p)`; unknown object → `false` | `object_role(o) == Dependency`; unknown object → `false` (kept) |
| `program/model/coverage.rs:243-250` | `is_none_or(eq_ignore_ascii_case)` | `routine_role` |
| `l5/digest.rs:3700-3712` (`reportable_roots`) | `is_some_and(!eq_ignore_ascii_case)` | `routine_role` |
| `l4/effect_query_cli.rs:355-360` | `is_none_or(eq_ignore_ascii_case(&a.app_guid))` | `routine_role` |
| `l5/selector_index.rs:80-95` | stable id starts with `<guid>:` (case-insensitive) | `role_of_stable_id` (the same test, moved to `role.rs`) |
| `l5/snapshot.rs:2433-2435` | stub `false` | `routine_role(r) == Dependency` |
| `l5/snapshot_full.rs:381-411` (`project_apps`) | primary app only, by guid | D3 |
| `l5/snapshot_full.rs:1280-1286`, `preflight.rs:25-30` | role = position `apps[0]` | the `role` field (D5) |
| `program/model/projection.rs:97-111` (11b `role_of`) | `ws:` unit = primary | **kept**; T1c |
| `l5/detectors/d1.rs:1993-1998` (O1) | every routine `"primary"`, passed to `pick_actionable_anchor` (`:620`, `:1572`), in cross-app runs too | **kept.** Converting it gives a d1 finding whose path ends in a dependency routine a primary actionable anchor, so it could survive `--scope primary` (`actionable_anchor.rs:30-55`, `registry.rs:727-745`) and move CDO SARIF. Filed as a discovery (ledger); A8.1's grep lists it as an expected hit |
| `l4/exec_relation.rs:353-359`, `l5/test_support.rs:635` (test helpers) | lowercase compare | `routine_role` |

Every "no primary app" arm agrees today: no primary ⇒ every row primary (the cross-app base
refuses a model without one, `capability_cone.rs:3866-3872`). Two absent-id defaults are
policies for an id the map does not hold, not role rules, and stay: `role_scope_and_sort`
reads an absent routine as primary (`registry.rs:733-736`); `pick_actionable_anchor` as
non-primary (`actionable_anchor.rs:34-35`). The converted sites `effect_query_cli`,
`selector_index`, `coverage` and `gate/run` have no use test of their own in this step: they
rest on T1 (the rule) plus the CDO identity runs of §8, which is enough only because each new
call is equivalent to the old comparison (P1b, P8).

### 3.2 Gaps today (`snapshot.rs:2359-2430`)

Three loops over `base.ws_routines`: `parse-incomplete` per flagged routine;
`symbol-only-boundary` per app guid (`object_id`'s first segment) of a body-less dependency
routine; `body-unavailable` per body-less dependency routine whose app is not in that set (dead,
§0.2). With the stub `false`, loops 2 and 3 emit nothing. `prove` counts a gap whose subject is
a visited routine id, or a bare app guid prefixing a visited stable id (`prove.rs:158-187`): a
per-root reach filter of its own.

### 3.3 The snapshot's top-level keys

Insertion order (`snapshot_full.rs:113-155`, load-bearing for CBOR): `schemaVersion`,
`alsemVersion`, `workspaceFingerprint`, `generatedAt`, `apps`, `identities`, `contractFacts`,
`schemaFacts`, `permissionFacts`, `rootClassifications`, `capabilityFacts`, `typedEdges`,
`operationIndex`, `callsiteIndex`, `callsiteResolutions`, `analysisGaps`, `coverage`, `inputs`,
`eventDeclarations`, then optional `inputsMetadata`, `routineOrderFrames`. The schema version
lives in two constants: `snapshot_full.rs:40` (pub) and `gate/snapshot_deserialize.rs:38`
(private copy). Facts keep INTERNAL ids for resources and witnesses (`snapshot.rs:1398-1406`):
`<guid>/table/N` for a table, `<guid>/table/N/F` for a field, a value source's `tableId`
(`snapshot.rs:85-91`); the identity table holds stable ids (`<guid>:Table:N`, `<guid>:<Type>:N`,
stable routine ids; `snapshot.rs:1035-1045`, `:1372-1396`).

## 4. Design

### 4.1 D1: `Role`, one rule

```rust
// src/program/model/role.rs (new)
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role { Primary, Dependency }
impl Role { pub fn as_str(self) -> &'static str { /* "primary" | "dependency" */ } }

impl Model {
    /// THE role rule. No primary app => every row is primary.
    pub fn role_of_app(&self, app_guid: &str) -> Role;   // eq_ignore_ascii_case
    pub fn routine_role(&self, r: &ModelRoutine) -> Role; // role_of_app(&r.app_guid)
    pub fn object_role(&self, o: &ModelObject) -> Role;   // role_of_app(&o.app_guid)
}

/// The same rule over a stable id's `<appGuid>:` prefix, for readers that hold
/// snapshot ids and no model rows (selector index, serializer, shards). P1b: the
/// prefix equals the row's app_guid on every CDO row and object.
pub fn role_of_stable_id(primary: Option<&str>, stable_id: &str) -> Role;
```

- A rule, not a store: a role is a function of the row's own `app_guid`, so no `Vec<Role>` is
  kept in sync (8A-4 withdraws the epic §5.5 `rel.role`).
- Every converted site of §3.1 calls it. `dep_routine_ids` stays as the string-keyed
  projection, built once from `routine_role` where the base builds it, and documented as such
  (8A-2).
- The registry's maps keep their `&str` values (`role_scope_and_sort` and
  `pick_actionable_anchor` read them); the value is `Role::as_str()`.

### 4.2 D2: gaps

- **In memory (every universe):** `derive_analysis_gaps(resolved, base)` (it gains `resolved`
  to reach the rule). `is_dependency_role` is `resolved.routine_role(r) == Dependency`.
  `SnapshotAnalysisGap` gains `role: String` (serialized last, after `detail`): `"primary"` for
  a primary `parse-incomplete`, else `"dependency"`. Kinds, subjects, sort and dedup unchanged.
  No reach filter here (I8): `prove` reads this table and intersects it with its own cone.
- **Serialized, cross-app only:** the Gap class (D4) keeps every primary gap and a dependency
  gap only if its subject is reachable: a routine subject (stable id) whose row is in `reach`,
  or an app subject (bare guid) owning a row in `reach` (same `object_id` first-segment
  extraction as loop 2, compared case-insensitively).
- **`reach`** = BFS over `rel.cone()` (`TypedEdge.from` → `to`, internal ids) from **every
  primary routine of the model** (Q4; I4: output never depends on which jobs share a view).
  Built once per cross-app serialization. P6: 17 ms on CDO (release-fast, indicative).
- **Opaque means source-less (O16).** In the cross-app serialization, where
  `dependency_apps[].has_source` is at hand, a `debug_assert!` checks that the app of every
  `symbol-only-boundary` gap has `has_source == false`. P5 shows it holds today; the assertion
  makes a future body-less row in an app with source fail loudly instead of reporting a false
  opaque boundary (F21).
- **Decision 8's gap half:** a primary call resolved into a body-less dependency row reaches
  that row, so its app's `symbol-only-boundary` is emitted. No separate emitter. T5 pins it.

### 4.3 D3: schema 4

- **One constant:** `snapshot_full::SNAPSHOT_SCHEMA_VERSION = 4`; the deserializer and the shard
  manifest use it; the private copy (`snapshot_deserialize.rs:38`) is deleted.
- **`analysisUniverse`** is inserted right after `generatedAt` (CBOR insertion order; raw JSON is
  sorted, so its place there is alphabetical). Value: `"single-app"` | `"cross-app"` (Q8: the
  flag's spelling; `"primary-only"` is already a shard mode, `alsem.rs:1233`). The envelope
  payload keeps it (`build_envelope` copies unknown keys, `snapshot_full.rs:1002-1011`).
- **`FullSnapshotOptions` gains `universe: SnapshotUniverse<'a>`:**
  ```rust
  pub enum SnapshotUniverse<'a> { SingleApp, CrossApp(CrossAppUniverse<'a>) }
  /// Field private: built only by `of`, so `rel`, `dependency_apps` and the
  /// model all come from ONE base (O18).
  pub struct CrossAppUniverse<'a> { base: &'a R3a5CrossAppBase }
  impl<'a> SnapshotUniverse<'a> {
      /// SingleApp → SingleApp; CrossApp(base) → CrossApp { base }.
      pub fn of(target: &'a AnalysisTarget) -> Self;
  }
  ```
  An options field, not a new parameter, so step 6's signature rewrite of
  `compose_full_snapshot` rebases over it without a conflict in the parameter list. The model
  is still a parameter, so in `CrossApp` `compose_full_snapshot` checks model identity, not
  just id membership: `assert!(std::ptr::eq(resolved, &base.resolved))` (always on; one
  pointer compare). Two revisions of one workspace share routine ids, so an endpoint check would
  pass for the wrong model; a pointer check cannot (O18, round 2). T12 pins it.
- **`apps[]`** rows: `appGuid, publisher, name, version, role`. Single-app: the primary row
  (today's), `role: "primary"`. Cross-app (8A-3): the primary row first, then every dependency
  app that owns a SERIALIZED `identities` row, sorted by `locale_compare` of guid (today's sort).
  Ownership is matched case-insensitively (O13): `DependencyApp.guid` keeps the manifest's
  spelling (`workspace.rs:1551`) and is written as is, so no single-app byte changes.
  `DependencyApp` gains `publisher` (from `AppId.publisher`, `program_calls.rs:1002-1007`);
  `R3a5CrossAppBase` keeps `dependency_apps` (cloned before `resolved_app_versions` is built from
  it, `capability_cone.rs:3991-3995`). A serialized identity whose app is not in
  `dependency_apps` gets no `apps[]` row, and a `debug_assert!` says it cannot happen: every
  dependency row comes from a required app (`program_calls.rs:995-1008`) (O14, F22).
- **Versions (Q3):** the capability-snapshot envelope keeps `schemaVersion "1.1.0"` (its payload's
  `snapshotSchemaVersion` carries the change). The routine-inventory document
  (`build_inventory_doc`, `snapshot_full.rs:1137-1233`) copies `apps` from the tree, so its rows
  gain `role`; its `payload` gains `analysisUniverse` (next to `apps`; the envelope keeps no
  snapshot field) (O8); and its `routineInventory` list, today built from every model routine
  (`:1165-1205`), is classed Inventory by `stableRoutineId`, so cross-app it lists primary
  routines only, the same rows as the filtered `identities` (S5, O8). `INVENTORY_SCHEMA_VERSION`
  goes `"1.1.0"` → `"1.2.0"` (additive, as the 1.0.0 → 1.1.0 bump was, `cli_p1_inventory.rs:91-93`).

### 4.4 D4: key classes in code

```rust
pub enum KeyClass { Header, Input, Inventory, Behaviour, Gap, Evidence }
/// Every top-level key, in insertion order, with its class.
pub const SNAPSHOT_KEYS: &[(&str, KeyClass)] = &[ /* table below */ ];
pub enum Domain { Identity, App, Operation, Callsite, Frame, Edge }
/// Per key: its defining field and its reference fields with their domains (table below).
pub const REFERENCES: &[(&str, Option<&str>, &[(&str, Domain)])] = &[ /* list below */ ];
/// Fields that are data, never references (list below).
pub const NON_REFERENCE_FIELDS: &[&str] = &[ /* list below */ ];
```

| Class | Keys (subject field) | Serialized rows, cross-app |
|---|---|---|
| Header (8A-1) | `schemaVersion`, `alsemVersion`, `generatedAt`, `analysisUniverse` | as is |
| Input | `workspaceFingerprint`, `inputs`, `inputsMetadata` | as is |
| Inventory | `contractFacts` (`stableId`), `schemaFacts` (`stableId`), `permissionFacts` (`subject`; `permissionSet` when `kind == "declared"`, as `filter_permission_facts`), `rootClassifications` (`routineId`), `eventDeclarations` (`routine`); `apps` (D3) | rows whose subject is primary |
| Behaviour | `capabilityFacts` (`subject`), `coverage` (`subject`), `callsiteResolutions` (`from`), `typedEdges` (`from`), `routineOrderFrames` (map key) | rows whose subject is primary; `routineOrderFrames` also keeps the frames of every kept Evidence row's `routine` (S2) |
| Gap | `analysisGaps` | D2 |
| Evidence | `operationIndex` (`routine`, id `operationId`), `callsiteIndex` (`routine`, id `callsiteId`), `identities` | rows of a primary routine, plus rows whose id is REFERENCED by a kept row; `identities` = every primary id plus every referenced id (Decision 17) |

**Typed rows, before CBOR (O7, decision).** The filter runs on the typed rows
`compose_full_snapshot` holds before it calls `to_cbor_value`: the local `core`
(`CapabilitySnapshot`, `snapshot_full.rs:85`) is the composer's own copy, so filtering its
vectors leaves every other in-memory snapshot unfiltered (I8). The three model-derived keys
(`derive_contracts`, `derive_schema`, `derive_permissions`, `:92-95`) skip dependency rows by
`object_role`/`routine_role` while deriving. Why this and not a filter over the finished
`CborValue` map: on CDO cross-app 26,179 of 31,658 routines are dependency rows (P6), and the
single-app fingerprint is already 129 MB at the base (P3); building their CBOR only to drop it
would load step 9's cross-app peak bound for nothing. Step 6 (scoped composition) must produce
exactly what this filter produces; T9 and T10 are its oracle.

**References, typed by domain (S1, O3, S9; round 2: SN1, SN2, SN3, ON1, ON3, ON4).** A row's
fields are of three kinds. A DEFINING field is the row's own id; it is never a reference, so a
row cannot keep itself (SN2). A REFERENCE field names a row of one DOMAIN. Every other field is
data and is never read (S9). The domains, each with one destination and one matching rule:

| Domain | Destination (and its class) | Values look like | Matching rule |
|---|---|---|---|
| identity | an `identities` row (Evidence) | stable routine/object/table ids; stable field ids `<guid>:Table:N#F`; stable event ids `<guid>:<Type>:<N>::<Event>::<hash>`; internal table/field/key ids `<guid>/table/N[/…]`; internal object-rooted ids `<guid>/<Type>/<N>[/…]`; internal routine ids `<instance>/<hash>` | a stable routine/object/table id as is; any other stable id → its object or table, the first three `:` segments (ON1; a field id → its table); an internal table/field/key id → `stable_table_id` of its first three `/` segments (`snapshot.rs:1035-1045`), `<guid>:Table:N`; another internal object-rooted id → `to_stable_object_id` of its first three segments (`ids.rs:135-137`); an internal routine id → `routine_to_stable` (`snapshot.rs:1029-1033`) |
| app | an `apps[]` row (Inventory) | a bare app guid (an app-level gap subject) | case-insensitive guid compare |
| operation | an `operationIndex` row (Evidence) by its `operationId` | `<instance>/<hash>/opN` | verbatim (SN1, ON4) |
| callsite | a `callsiteIndex` row (Evidence) by its `callsiteId` | `<instance>/<hash>/csN` | verbatim (SN1, ON4) |
| frame | the `routineOrderFrames` table of the referring row's `routine` (Behaviour) | `order.frameId`, an integer | the frame id inside that routine's table |
| edge | a `typedEdges` row (Behaviour) by its `edgeId` | edge ids | verbatim |

`REFERENCES: &[(key, defining_field, &[(field_path, Domain)])]` holds, per key (field paths
from the serialized rows, `tests/cli-b-goldens/snapshot/*.raw.json`, and the structs of
`snapshot.rs:270-1000` AND the model-derived rows of `snapshot_full.rs:417-812`, SN3):
- `contractFacts`: defines `stableId`; no references.
- `schemaFacts`: defines `stableId`; no references.
- `permissionFacts`: `subject`, `target` (SN3), `permissionSet` → identity;
  `derivedFromCapability.witnessCallsiteId` → callsite; `derivedFromCapability.witnessFirstHopOperationId` → operation (S3N2, `snapshot_full.rs:795-800`); `derivedFromCapability.witnessFirstHopEventId` → identity.
- `rootClassifications`: `routineId`, `sourceAnchor.enclosingRoutineId` → identity.
- `eventDeclarations`: `routine`, `eventId`, `binding.publisherObject`, `sourceAnchor.enclosingRoutineId` → identity.
- `capabilityFacts`: `subject`, `resourceId`, `witnessFirstHopEventId` → identity; `witnessOperationId`, `witnessFirstHopOperationId` → operation; `witnessCallsiteId` → callsite; `resourceArgSource`, `extra.bodyArgSource`, `extra.keyArgSource`, `extra.valueArgSource` → value source (below).
- `coverage`: `subject`, `unknownTargets` → identity.
- `callsiteResolutions`: `from`, `candidates`, `unresolvedCandidates.objectId` → identity; `callsiteId` → callsite; `resolvedEdges` → edge.
- `typedEdges`: defines `edgeId`; `from`, `to`, `eventId`, `targetObject`, `*Anchor.enclosingRoutineId` → identity; `callsiteId` → callsite; `operationId` → operation; `targetIdSource` → value source (below).
- **Value source** (`SnapValueSource`, `snapshot.rs:65-95`; the fields above, `snapshot.rs:167-174`, `:347`, `:510`): read by its `kind`. A `table-field` source's `tableId` → identity; a `constant-var` source's `initializer` is a value source again, read recursively; every other part (`value`, `enumName`, `member`, `varName`, `index`) is data (S3N2).
- `analysisGaps`: `subject` → identity (a stable routine id) or app (a bare guid).
- `operationIndex`: defines `operationId`; `routine` → identity; `order.frameId` → frame.
- `callsiteIndex`: defines `callsiteId`; `routine` → identity; `order.frameId` → frame.
- `routineOrderFrames`: defines its map key (and each frame its `frameId`); `parentFrameId` → frame (same table), except the root frame's `-1`, the one documented sentinel (`operation_order.rs:45-50`, `:69-73`; every `parentFrameId` in the cli-b goldens is `-1` or a frame of the same table).
- `identities`: defines `stableIds`; no references.

Every other field is listed as data in `NON_REFERENCE_FIELDS` (ON3), at least: `value` (a
literal value source), `displayText`, `calleeDisplay`, `sourceFile`, `sourceUnitId`,
`extra.recordVariableId`, `detail`, `reasons`, `attributes.*`, `signatureFingerprint`,
`shapeFingerprint`, `kind`, `op`, `rights`. The implementation completes both lists from the
same census; a field in neither list fails T9-closure.

**Order of the filter steps (O17).** (1) Inventory, Behaviour and Gap rows are filtered by
subject. (2) Their references are resolved. (3) Evidence index rows (`operationIndex`,
`callsiteIndex`) are kept if their routine is primary or their id is referenced. (4) The kept
Evidence rows' references are resolved; each kept row's frame table joins `routineOrderFrames`.
(5) `identities` keeps every primary id and every referenced identity. (6) `apps` keeps the
primary row, every app owning a kept identity, and every app referenced by a kept gap. Steps
(2) and (4) resolve against the UNFILTERED rows.

**Step (6) for a guid with no app metadata (O4N1, decision).** An app guid that a kept gap
references but that is neither the primary nor in `dependency_apps` gets NO `apps[]` row: step
(6) has no publisher, name or version to build one from, and it never guesses. It does not
assert either. The Closure check below then reports it as an app-domain violation, which is
what T9-closure's dangling-app arm expects (a reported violation, not a panic). This differs
from F22's identity assertion on purpose: a gap subject comes from the in-memory gap table,
which a test can hand-state, so the check must report it, not abort the composition. In
production the case cannot arise (a gap's app guid comes from a model row, and every model row
comes from the primary or a required app, `program_calls.rs:995-1008`); F31 covers it if it does.

**Closure (checked per domain; S3N3).** After step (6), every reference of every serialized row
resolves to a serialized row of its domain. The only exceptions are explicit:
- **identity** values may stay unresolved only in these shapes (`EXTERNAL_IDENTITY_SHAPES`): the
  literal `"unknown"`; and an id rooted in a guid that is neither the primary nor a
  `dependency_apps` guid (an app outside the model, such as a package the workspace never
  required) and equal to no model routine id. An identity value rooted in a model app, or equal
  to a model routine id, that resolves to nothing is a violation: it is a mapping error. This is
  why a wrong table mapping fails by itself (SN4): `bbbb…/table/18` rewritten to
  `bbbb…:table:18` names a model app and resolves to nothing.
- **frame**: `parentFrameId = -1` (the root frame) is no reference.
- **operation, callsite, edge, app** have no external shape: a value that resolves to no row
  is a violation, whatever it looks like.
A new external shape enters the list only through F25 (a recorded discovery), never as a
general relaxation. T9-closure asserts the rule on DD and R5B and breaks each domain once.

**Sharding (S3, O4, O13).** `slice_for_app` (`snapshot_full.rs:1318-1376`) keeps its own
per-key membership match (`routine` for the indexes, `from` OR `to` for edges, `routine` OR
`binding.publisherObject` for event declarations, verbatim copy for header, `inputs`,
`analysisGaps`): it decides shard membership, not role. Two changes: its `<guid>:` prefix
compare becomes case-insensitive (O13), and after slicing each shard is CLOSED (S3) over the
identity, operation, callsite and edge domains: every row of the full tree that a kept shard
row references in one of those domains (an `identities` row, an `operationIndex` or
`callsiteIndex` row, a `typedEdges` row) is added to the shard, and the rows so added are
scanned in turn until nothing new is added (a fixpoint; each pass only adds rows of the finite
full tree). **The frame and app domains are NOT closed in shards** (S3N1, O3N1): a shard has
never carried `routineOrderFrames` (`slice_for_app` skips the key, `snapshot_full.rs:1326-1333`,
as al-sem's `sliceForApp` did; the three committed `primary.json` shard goldens carry
`order.frameId`s and no `routineOrderFrames`), and it copies `analysisGaps` whole while keeping
one `apps[]` row. Closing them would change the shard shape. So a shard satisfies the Closure
rule for the four closed domains only; the full snapshot keeps the whole rule. In single-app
every identity, Evidence or edge row a shard reference reaches is primary and already in the
shard (P3; the slice keeps every primary row), so nothing is added and no shard golden moves
beyond the allow-list's header and `role` changes (F26 if one does). T9-keys checks
that the keys `slice_for_app` matches equal `SNAPSHOT_KEYS`; nothing panics at run time (O4).

**Key-set equality (T9-keys):** both universes compose keys ⊆ `SNAPSHOT_KEYS` with every
required key present. In `SingleApp` the filter returns at once (every row is primary, P3), so
the single-app path is today's plus the header and `role` fields.

### 4.5 D5: preflight, shards, exit code

1. **Schema 3 refused at load.** `deserialize_snapshot` already refuses any version but the
   constant. The message gains the remedy: `deserializeSnapshot: snapshot schemaVersion 3 is not
   supported by this build (v4); re-create it with 'alsem fingerprint'`.
2. **Universe and roles validated at load (O6, S4, S14).** A v4 file is refused (exit 1, the
   load-failure path P9 measured) when `analysisUniverse` is missing or not one of the two
   values; when an `apps[]` row has no `role` or an unknown one; or when `apps[]` is non-empty
   and does not hold exactly one `role: "primary"` row. A dependency shard (its `apps[]` is one
   `dependency` row) is refused by the last rule with the hint `diff the full snapshot or the
   primary shard`. An empty `apps[]` (no `app.json` id) stays accepted, as today.
3. **Universe mismatch.** `run_preflight` adds a fatal `analysis-universe-mismatch`
   (`kind`, `old`, `new`), checked right after `schema-version-mismatch`, same shape as
   `app-identity-mismatch`.
4. **Identity by role.** `primary_app_id` reads the `apps[]` row with `role == "primary"`;
   `serialize_sharded` reads each row's `role` (`primary-only` keeps `role == "primary"`).
   `apps[0]` is never read for a role. The manifest shape is unchanged.
5. **A fatal preflight exits 1 (decision, P9).** `DiffEngineResult` gains `fatal: bool`;
   `diff/cli.rs`'s exit gate sets 1 when it is true, before the coverage and `--fail-on`
   gates. Stated behaviour change: `app-identity-mismatch` and strict `alsem-version-mismatch`
   also exit 1 now (they exited 0). `schema-version-mismatch` is reachable only from in-process
   trees, since the loader refuses any other version first; it is listed for completeness
   (O12). stdout is unchanged. Why in step 8: E8's "refused" means nothing to CI if the refusal
   exits 0.

### 4.6 D6: the vendored diff inputs

The 12 `tests/cli-b-goldens/diff/snapshots/*.snap.json` are INPUTS at schema 3, "never
regenerated" (`diff/PROVENANCE.md`). Q7: they are upgraded by hand with Edit, three edits each
(`schemaVersion` 4; `"analysisUniverse": "single-app"`; `"role": "primary"` on the one `apps[]`
row), and PROVENANCE gains one paragraph saying so. Every diff OUTPUT must stay byte-identical.
The inline snapshots of `cli_b_diff_differential.rs` (`snapshot()`, `:489-500`) use the
constant, the universe key and a `role` on their app rows; the schema-mismatch oracle
(`:545-566`) keeps its 99 sentinel on the old side and uses the constant on the new side
(`:551`).

### 4.7 What is deleted

The stub, the private schema constant, the lowercase/`eq_ignore_ascii_case` copies of §3.1 that
are converted, `R3a5CrossAppBase::primary_app_guid` and its census bucket `base.primary_app_guid`
(no test reads it; stderr census only, stated). Fallback F13 if something does read it.

## 5. Test plan (every new test: break, failing assertion, revert; I9)

Fixtures: **DD** = `tests/common/dep_dispatch_fixture.rs` (`write_own`; embedded-source
dependency `XBase`; `CallChain → Outer → Inner`, where `Outer → Inner` is a dependency caller's
call, cone-only; `TouchCust` inserts dependency table `Customer` (18)). **R5B** =
`tests/r2-5b-fixtures/cross-app-resolution` (primary `1111…00aa`; two symbol-only apps,
`dddd…0001` reachable through `Host Mgt.DriveDeps`, `eeee…0002` not; P6). **R5B-F** = a temp
copy of R5B whose `app.json` id is `ffffffff-0000-0000-0000-0000000000aa`, so the primary sorts
LAST by guid. Cross-app models come from `AnalysisTarget::build(inputs, false, ..)` and the
composition from `compose_full_snapshot` with `SnapshotUniverse::of(&target)`. New integration
tests live in a new `tests/cli/cli_snapshot_schema4.rs` member; lib tests in `role.rs`,
`projection.rs` (T1c: `role_of` is private), `snapshot_full.rs` and `gate/diff/preflight.rs`.
"Hand-stated" = a field set by assignment on the assembled model before composing (CLAUDE.md).

| # | Test | Precondition (asserted) | Break → fails |
|---|---|---|---|
| T1 | `role_rule_hand_stated` (lib): primary guid upper and lower case, a dependency guid, `primary_app: None`; `role_of_stable_id` with `<PRIMARY>:x`, `<dep>:x`, `<primary>x` (no colon) | rows built by assignment | case-sensitive compare → upper-case row `Dependency`; drop the `None` arm → `Dependency`; drop the `:` check → `<primary>x` reads `Primary` |
| T1c | `projection_role_of_agrees_with_rule` (lib, `projection.rs`; R5B plus an embedded-source dependency in a temp dir, as 11b's tests write one) — supplementary, not use coverage (S13) | both kinds of dependency row present (symbol-only, `dep:` source) | for every row, `role_of(r)` is `None` exactly when `routine_role(r) == Primary`, and otherwise `Some` of the row's lowercase `app_guid` (the full value, app included). Break 1: `role_of` treats `dep:` as primary → fails. Break 2: `role_of` drops the lowercase step → an upper-case manifest guid fails |
| T2 | `cross_app_scope_uses_role` (lib): a dependency-object-anchored finding and a dependency-routine-anchored one are dropped under `--scope primary`, kept under `--scope all`; a `dep:`-unit finding with a primary actionable anchor is kept | the model holds both dependency rows | `role_of_app` → always `Primary` → both kept, fails. The existing `dep:` test stays |
| T3 | `gaps_reachable_only` (R5B) | in memory: `symbol-only-boundary` for BOTH apps (each `role: "dependency"`); `eeee…` rows are model rows, none in `reach` | serialized: `dddd…` present, `eeee…` absent. Break 1: skip the reach filter → `eeee…` appears. Break 2: stub back to `false` → `dddd…` missing. Both directions of Decision 46 |
| T4 | `parse_incomplete_gap_filter` (temp fixture: embedded-source dependency with a routine `A` a primary routine calls, and a reverse-demand raiser `R` that raises a dependency event a primary routine subscribes to; a primary routine `W`) | `R` is a model row not in `reach`; `A`, `R`, `W` flagged `parse_incomplete` (hand-stated if the source cannot flag a dependency row, F10) | serialized: `W` (role `primary`) and `A` gapped, `R` not. Break 1: no reach filter → `R` appears. Break 2: hard-code `role: "dependency"` → `W`'s `role == "primary"` fails (S7: `W` is a BFS root, so only the annotation can discriminate the primary arm) |
| T4c | `gap_reach_follows_dependency_calls` (DD, cross-app; O2, S6) | `Inner` hand-stated `parse_incomplete`; `Inner` is reachable over the cone and NOT over the cut (asserted by computing both) | serialized `parse-incomplete` gap for `Inner`. Break: BFS over the cut (`rel.primary()` edges) → the gap disappears. This is the F9 shape P6 measured |
| T5 | `symbol_only_callee_has_its_gap` (R5B) | a typed edge from `DriveDeps` targets a `dddd…` body-less row (asserted on `rel.cone()`) | serialized `symbol-only-boundary` for `dddd…`, `role: "dependency"`. Break: stub back to `false` → gap missing. (The cone-vs-cut discrimination is T4c's: this edge is in both graphs, S6/O2.) Step 6 adds: the call's ledger row is `unfetched-dependency` (F7) |
| T6 | `diff_refuses_mixed_universes` (cli): two v4 files, same app, `single-app` vs `cross-app` | both load | `analysis-universe-mismatch` in the diagnostics, no findings, exit 1. Break: drop the check → findings computed, exit 0 |
| T6b | `fatal_preflight_exits_one` (cli): two vendored inputs of different apps | both load | exit 1 with `app-identity-mismatch`. Break: drop the `fatal` gate → exit 0 (P9) |
| T7 | `diff_refuses_schema_3` (cli): a temp copy of a vendored input edited back to `schemaVersion: 3`, diffed against a v4 file | the copy carries 3 | stderr names 3, v4 and the remedy; exit 1. Break: accept 3 → loads, fails |
| T7b | `diff_refuses_bad_universe_or_roles` (cli, S14, O6): five temp copies: no `analysisUniverse`; `analysisUniverse: "both"`; an `apps[]` row without `role`; two `primary` rows; a dependency shard (`apps[]` = one `dependency` row) | each copy differs from a valid v4 file in that one field (asserted) | each refused at load, exit 1, the message names the field (the shard case names the hint). Break: drop each check in turn → that copy loads |
| T8 | `shards_read_role_not_position` (lib, `snapshot_full.rs`; O15): a hand-built v4 tree whose `apps[]` lists a dependency (`0000…`) before the primary | hand-built, not composed | manifest roles right; `primary-only` writes the primary's shard only; the tree also holds a primary fact whose `witnessOperationId` is a dependency operation row (with an `order.frameId`, and its routine's frame table in `routineOrderFrames`), and a primary edge to a dependency routine; the primary shard holds that operation row and both dependency identities, and still has no `routineOrderFrames` key (S3, S3N1, O3N1). Break 1: read `apps[0]` → roles swapped. Break 2: close identities only (round 1's rule) → the operation row is missing. Break 3: one pass instead of a fixpoint → the operation's routine identity (reached only through the added operation row) is missing. Break 4: close the frame domain too → the shard gains `routineOrderFrames`, fails |
| T8b | `preflight_identity_reads_role` (lib, `preflight.rs`; O15) | same tree shape, two sides of different primaries, same `apps[0]` | `app-identity-mismatch` raised. Break: read `apps[0]` → no diagnostic |
| T9a Inventory | DD cross-app | in memory, `contractFacts` holds `Dep Lib`/`Dep Sub` rows, `schemaFacts` holds dependency table `Customer` (18) rows (S15), `rootClassifications` and `eventDeclarations` hold dependency rows, `routineInventory` lists `Outer` (each precondition asserted per key) | serialized: none of them; inventory doc's `routineInventory` primary only. Break: identity filter → they appear |
| T9b Behaviour | DD cross-app | in memory, `Inner` has direct capability facts and coverage | serialized: no `capabilityFacts`/`coverage`/`typedEdges`/`callsiteResolutions` row with a dependency subject; `CallChain`'s rows present. Break: identity filter → `Inner` appears |
| T9c Gap | T3, T4, T4c | — | — |
| T9d Evidence | DD + R5B cross-app (S8, O3) | `Outer` is referenced by `CallChain`'s typed edge; `TouchCust`'s fact names `bbbb…/table/18`; R5B's `eeee…` rows are unreferenced; in memory, `Inner`'s `operationIndex` row and `Outer`'s `callsiteIndex` row exist | serialized `identities` holds `Outer`, `bbbb…:Table:18` and every primary id, no `eeee…` id; `operationIndex` holds `CallChain`'s and `TouchCust`'s rows and NOT `Inner`'s (unreferenced on the cut); `callsiteIndex` holds `CallChain`'s rows and NOT `Outer`'s. Break 1: emit every row → `eeee…` ids and `Inner`'s operation appear. Break 2: primary only → `Outer` missing. Break 3 (moved to T9-closure, SN4: `TouchCust`'s permission `target` already names `bbbb…:Table:18`, so this identity survives a wrong table mapping here). Break 4: drop primary rows from Evidence → `TouchCust`'s operation missing |
| T9g | `evidence_follows_dependency_witness` (lib, `snapshot_full.rs`, hand-built typed rows through the production filter; SN1) | a primary fact's `witnessOperationId` and `witnessCallsiteId` name a dependency routine's operation and call-site rows, each with an `order.frameId` in that routine's frame table; that routine is referenced by nothing else | the filtered output keeps both Evidence rows, the routine's frame table and the routine's identity. Break 1: match operation ids through the identity mapping (object prefix) instead of verbatim → the rows are dropped. Break 2: skip filter step (4) → the frame table and the identity are missing |
| T9-closure | DD + R5B cross-app; one hand-stated literal (S9, SN2, SN4, ON3, ON4) | DD's `TouchCust` fact carries `resourceId` `bbbb…/table/18`; a `literal` value source in a kept fact carries a dependency stable id as its `value` (hand-stated); a kept fact carries a `table-field` value source with a `tableId` (hand-stated if none, asserted) | the per-domain Closure of §4.4 holds; every string that is id-shaped (rooted in a model app guid, or equal to a model routine id or an Evidence row id) sits under a field of `REFERENCES` or `NON_REFERENCE_FIELDS`; the literal keeps nothing. Break 1: read every `Text` → the literal keeps its id. Break 2: drop `tableId` from `REFERENCES` → it is in neither list, fails. Break 3: map internal tables by a `/`→`:` rewrite → `bbbb…:table:18` is model-rooted and resolves to nothing, a violation. Break 4: treat `stableId` as a reference → T10's unreferenced-table precondition fails (SN2). **Nested references (S3N2)**, each its own arm on a hand-stated kept primary fact, each naming a dependency table referenced nowhere else (asserted): a `tableId` inside `extra.bodyArgSource`, one inside `extra.keyArgSource`, one inside `extra.valueArgSource`, one inside a `constant-var` `initializer` two levels deep, and a `derivedFromCapability.witnessFirstHopOperationId` naming a dependency operation; each arm's identity or operation row is serialized. Break 5: read only `resourceArgSource` → the three `extra` arms fail; break 6: no recursion into `initializer` → the nested arm fails; break 7: drop `witnessFirstHopOperationId` from `permissionFacts` → its operation row is dropped, a violation. **Per-domain resolution (S3N3)**, one hand-stated dangling value per domain in a kept row (an unknown `operationId`, `callsiteId`, `edgeId`, an `order.frameId` absent from its table, a gap subject guid that owns no row): each is reported as a violation; break 8: allow any unresolved value in that domain as external → that arm passes silently, fails. `parentFrameId = -1` is not reported (asserted on DD's root frames) |
| T9e Input | DD single vs cross | — | `workspaceFingerprint`, `inputs` equal across universes. Break: filter `inputs` as Inventory → dependency `.app` rows vanish |
| T9f Header and order | DD both universes; R5B-F (S8) | R5B-F's primary guid sorts after `dddd…` (asserted) | `analysisUniverse` per universe; DD cross-app `apps` holds `bbbb…` with `role: "dependency"`, publisher `probe`; R5B-F `apps[0]` is `ffff…` with `role: "primary"`, `dddd…` second, no `eeee…` row. Break: sort `apps[]` by guid only → `dddd…` first |
| T9-keys | both universes, DD (O4) | — | composed keys ⊆ `SNAPSHOT_KEYS`, required keys present; the keys `slice_for_app` matches equal `SNAPSHOT_KEYS`. Break: insert a new key without a class → fails |
| T10 | `no_dependency_cross_equals_single` (a temp copy of `tests/r0-corpus/ws-d8-commit-in-tx` plus one file `src/Unused.al` declaring table 50199 `Unused` that no code touches; S7, O5, S10, SN2) | the cross-app base builds with no dependency; one primary routine hand-stated `parse_incomplete` in both models, so `analysisGaps` is non-empty; table `Unused` is an `identities` row that no `REFERENCES` field of any row names (asserted; in the committed fixture every table, 36/37/50100, is a permission target, so none would do) | full trees equal except `analysisUniverse`. Break 1: write gap `role` only in cross-app → the single-app gap row lacks it, fails. Break 2: Evidence keeps referenced ids only (drop "every primary id") → the unreferenced table vanishes cross-app, fails |
| T12 | `cross_app_universe_binds_its_model` (cli member, O18) | two cross-app targets built from the same workspace (equal routine ids, asserted) | composing target A's model with `SnapshotUniverse::of(&target_b)` panics with the model-identity message; A with A composes. Break: drop the `ptr::eq` assert → it composes silently |
| T11 | `cli_p1_inventory` (edited) | — | inventory doc `schemaVersion "1.2.0"`, `apps[0].role == "primary"`, `payload.analysisUniverse == "single-app"`; the projection-subset check (`:105-125`) still byte-equal |

Tests rewritten with the code they pin (I2's exception): `snapshot_deserialize.rs`
`deserialize_json_raw`, `deserialize_json_enveloped`, `normal_gz_snapshot_round_trips_under_cap`,
`roundtrip_*` (they build `Int(3)` trees; now the constant + `analysisUniverse` + roles);
`wrong_schema_version_errors` gains 3 in its refused list; `cli_b_diff_differential.rs`
`snapshot()`, `oracle_preflight_schema_version_mismatch_is_fatal` (`:551`, new side) and
`oracle_preflight_app_identity_mismatch_is_fatal` (its app rows gain `role`). Compile-only
changes for the new `universe` field of `FullSnapshotOptions` (O11):
`tests/cli/cli_b_fingerprint_differential.rs:147`, `tests/cli/cli_b_snapshot_differential.rs:163`,
`tests/cli/cli_b_diff_differential.rs:201`, `tests/gap/gap_issue100_testfield_capability.rs:82`.
Any other lib test that reads in-memory gaps of a cross-app model is named in the plan (F11).

## 6. Acceptance matrix

| # | Issue / epic item | Evidence |
|---|---|---|
| A8.1 | `Role` with the `dep:` filter beside it | D1; T1, T1c, T2. Grep (O10): `rg -n 'app_guid.*(eq_ignore_ascii_case\|to_ascii_lowercase)\|eq_ignore_ascii_case\(g\)\|eq_ignore_ascii_case\(p\)' src` hits `role.rs` and, besides it, only these guid normalisations, none a role decision (ON2; measured at `f96619ec`): `program/resolve/compiler_oracle.rs:313`, `program/model/events.rs:254`, `program/model/program_calls.rs:386`, `engine/l5/detectors/d13.rs:82-83` (friend lookup), `program/model/program_calls_adapter_tests.rs:1505,1641` (test filters). Every other base hit (`coverage.rs`, `registry.rs`, `gate/run.rs`, `exec_relation.rs`, `test_support.rs`, `effect_query_cli.rs`, `selector_index.rs`, `digest.rs`, `capability_cone.rs`) must be gone; a hit inside 11b's `projection.rs:97-111` is the kept rule (T1c). d1's map holds no comparison, so it is checked by name: `d1.rs:1993-1998` unchanged (O1) |
| A8.2 | Live `is_dependency_role` with reachability | D2; T3, T4, T4c, T5 |
| A8.3 | Key classes | D4; T9a-g, T9-keys, T9-closure, T12 |
| A8.4 | Snapshot schema 4 | D3; `rg 'SNAPSHOT_SCHEMA_VERSION: i64' src` = 1 hit |
| A8.5 | `apps[]` by role | D3, D5.4; T8, T8b, T9f |
| A8.6 | Preflight | D5; T6, T6b, T7, T7b, T8b |
| A8.7 | Every cli-b diff checked against an allow-list (header, `apps[]` roles, gap role annotation; anything else fails) | §8 Part 0: `allowlist.py` over all of `tests/cli-b-goldens`, exact values and key order, fails closed; the 12 diff inputs as D6's three edits; `diff/PROVENANCE.md` as the one reviewed text change; every diff OUTPUT byte-identical |
| A8.8 | Tests: reachable-only gap; mixed-universe refusal; shards with a dependency `apps[0]`; one test per key class, gaps in both directions | T3 (+T4, T4c, T5); T6 (+T6b, T7, T7b); T8, T8b; T9a-f (gaps both directions = T3's two breaks) |
| A8.9 | Golden "every cli-b file" | **narrowed, with the reason.** Moves: the 62 schema-3 JSON files (P4), the 86 CBOR files, and `diff/PROVENANCE.md`. Does NOT move: digest, prove, fingerprint-query JSON, every `.human.txt`, every diff output (none carries the header, `apps` or gaps, P4). Part 0 fails if any of those changes |
| A8.10 | Golden `cli_p1_inventory` | T11 (test code: version 1.2.0, `role`, `analysisUniverse`) |
| A8.11 | Golden `cli_workspace_fingerprint_closure` | **no change expected**: it reads `workspaceFingerprint`, `inputs` and digest's dependency list (`:37-80`); the fingerprint hashes inputs and `alsemVersion` only (`snapshot_full.rs:894-908`). It must pass unchanged; a failure is a defect (F5) |
| A8.12 | "the key list in `tests/differential.rs` (~1758-1799)" | **not deliverable as written, because** those lines are R2d's `L3COV_FORBIDDEN_KEYS` (`:1781`), the L3 coverage surface's forbidden keys, not a snapshot key list (P10). `analysisUniverse` never enters that surface, so nothing there changes. The guard the citation aims at ("every snapshot key has a class") is T9-keys over `SNAPSHOT_KEYS` (8A-6) |
| A8.13 | Non-speed (Decision 58): no bench | §8: `cdo-gate` + identity only |
| E6 (8's half) | cross-app prove never falsely PROVEN | T3, T5: the reachable opaque boundary is a gap row with a bare app guid, which `count_gaps_in_cone` matches (`prove.rs:174-187`); step 9 wires prove |
| E7 (8's half) | every key classed | T9-keys |
| E8 | mixed universes / schemas refused | T6, T6b, T7, T7b |
| E14 / I10 | CDO ratchets | `scripts/cdo-gate` green |
| I2 (scoped) | what stays byte-identical | the 7 golden targets other than `--test cli`; in `--test cli`, every family except the cli-b files of A8.9 and `cli_p1_inventory`'s assertions; `cargo test -p al-sem --lib` outcomes apart from §5's named rewrites; CDO SARIF (4 runs), digest and histogram SHA |
| I4 | output independent of the job set | reach roots = every primary routine (D2) |
| I8 | serialized inventory primary-only, in-memory unfiltered | D2, D4 (the composer's local core only); T9a; the in-memory preconditions of T3/T9a |
| I9 | discrimination | every row of §5 |

Untouched: I1 (no build counter moves; the BFS is not one), I3, I5, I6, I7, I11.

## 7. False-positive and failure shapes

- **A site whose rule differs from today's:** P1b/P1c/P8 show none on CDO; d1's map is left as
  is (O1); Part 1's SARIF and digest identity catch a slip (F8).
- **A collided string id with two roles:** 11b's assertion (`projection.rs:48`), green on CDO (P7).
- **Demand churn as a false gap or false behaviour row:** T3's negative arm; T9a/b.
- **A gap hidden by over-filtering (later a false PROVEN):** T3's positive arm, T4c, T5.
- **Reach computed over the cut instead of the cone:** T4c (on CDO the cut reaches 361 of
  22,937 rows, P6).
- **A false opaque boundary for an app with source:** the D2 assertion (F21).
- **Reach from the job's subjects:** D2's roots take no job input (the composition has no
  subject parameter at step 8), so this shape cannot arise yet; step 12's batch contract test
  is where a job-dependent root would show.
- **A dangling reference (filtered too much):** T9-closure (per domain), T9d's break 2, T9g,
  T8's shard arms. **A row kept by an id-shaped literal or by its own id (filtered too
  little):** T9-closure's breaks 1 and 4.
- **A cross-app universe composed with another revision's model:** T12.
- **Position read as role:** T8, T8b, T9f (R5B-F).
- **A key added later without a class:** T9-keys.
- **A silent mixed-universe, stale-schema or role-less diff:** T6, T6b, T7, T7b.
- **The CDO run proves nothing about gaps:** stated limit (§0.6); the fixtures carry it.

## 8. Measurement plan (Decision 58: non-speed, no bench)

Run once after the final panel by the conductor. `P` = the commit step 8 is based on (after a
rebase, that base), `H` = head. `WS` = the pinned CDO workspace (`.agent/config.json`), never
written into a file.

- **Part 0, the cli-b allow-list (repo).** After `scripts/check-goldens --regen` on `H`:
  `python3 -I /work/bench/160-8/allowlist.py <P-tree>/tests/cli-b-goldens <H-tree>/tests/cli-b-goldens`
  exits 0. It is step 8's check of step 8's own diff, nothing else (S16). It decodes CBOR with
  `cbordec.py` (P4b) into ordered maps and parses JSON with key order kept (S12, O9). Rules:
  - The changed files must be exactly A8.9's list; `diff/PROVENANCE.md` is the one text file
    allowed to change, and its diff must be one added paragraph (S11).
  - **Exact values, not erasure (S12):** on `H` every snapshot, envelope payload and shard has
    `schemaVersion`/`snapshotSchemaVersion` = 4 (the manifest's `schemaVersion` = 4),
    `analysisUniverse` = `"single-app"`, every `apps[]` row `role` = `"primary"` (one row), and
    every gap row (if any) a `role`. After removing exactly those keys, `H` equals `P`.
  - **Key order (O9):** for every map, the sequence of keys on `H` minus the added keys equals
    `P`'s sequence; in CBOR and insertion-order JSON (`manifest.json`) `analysisUniverse`
    comes right after `generatedAt` and `role` last in its row.
  - The 12 diff inputs: `H` minus D6's three edits equals `P`.
  - It exits 2 on a missing or unparseable file, and on any other difference.
  - **The checker is mutation-tested before use:** four hand-made `H` copies (a wrong universe
    value, a missing `role`, a moved key, an unlisted changed file) must each exit 2. The
    script's SHA-256 and its output are recorded in the step's numbers file.
- **Part 1, identity (CDO).** `alsem analyze <WS> --format sarif` at `--scope primary` and
  `--scope all`, default (cross-app) and `--single-app`: `P` vs `H` byte-identical (4 pairs).
  `aldump --program-call-graph-stats <WS>` SHA equal. `alsem digest <WS> --routine <a primary
  root> --format json --deterministic` `P` vs `H` byte-identical (the cross-app in-memory
  snapshot now carries live roles; nothing it outputs may move). `alsem query effects` on one
  primary routine `P` vs `H` identical (the converted `effect_query_cli` site). `bash
  scripts/cdo-gate` green.
- **Part 2, reported only.** `alsem fingerprint <WS> --format json --deterministic` `P` vs `H`
  through `allowlist.py` (single-app CDO: header and `apps[0].role` only; 129 MB at `P`). The
  cross-app gap census is 0 on CDO by P5; reported as such, not re-run.

## 9. Amendments this spec asks the epic to take

- **8A-1** A sixth class, **Header** (`schemaVersion`, `alsemVersion`, `generatedAt`,
  `analysisUniverse`). §6 leaves the header unclassed, which Decision 13 forbids.
- **8A-2** `dep_routine_ids` stays as the string-keyed projection of the rule; its 71 reads are
  not re-keyed. "Replaces the base's id set" (§6) reads as "the set is derived from the one
  rule".
- **8A-3** `apps[]` is classed Inventory **of the universe**: the primary row first, then every
  dependency app owning a serialized `identities` row, each with `role`. §6 lists `apps` under
  "primary rows" and also asks for "`apps[]` primary first by role"; both cannot hold with
  primary rows only. Single-app: the primary row alone (P3).
- **8A-4** §5.5's `rel.role: Vec<Role>` (`Dependency { app }`) is withdrawn for a rule over the
  row's own `app_guid`.
- **8A-5 (replaces the pre-draft's)** Decision 8's gap is the app-level `symbol-only-boundary`:
  body-less dependency rows are exactly symbol-only apps' rows (P5), so no per-routine
  `body-unavailable` row arises in any model the program builds; D2 asserts it.
- **8A-6** Row 8's golden "the key list at `tests/differential.rs:1758-1799`" is withdrawn
  (P10, A8.12); T9-keys is the snapshot key-list guard. Also: Evidence (§6) keeps the rows of
  primary routines as well as the referenced ones, and `routineOrderFrames` keeps the frames
  its kept Evidence rows point at (S2); otherwise a no-dependency cross-app snapshot would
  differ from the single-app one, against §6's own contract.

## 10. Fallbacks (every "if X", pre-stated)

| # | If | Then (no new panel round unless it moves output) |
|---|---|---|
| F1 | P1b disagreed | **not fired** (0). Had it: the selector index calls the rule via the identity row |
| F2 | 11b's homogeneity assertion fired on CDO | **not fired** (P7). Had it: stop, a per-id role is a merge |
| F3 | a single-app snapshot names dependency apps | **fired in letter** (eventDeclarations, P3); `apps[]` reads `identities`, which is primary-only single-app, so nothing moves |
| F4 | converting a site needs edits in > 40 files | **not fired** (14 files, not re-keyed) |
| F5 | `allowlist.py` finds a non-allowed difference, or `cli_workspace_fingerprint_closure` fails | a defect; nothing merges until it is explained in source |
| F6 | 8A-5 contested | **taken by evidence** (P5) |
| F7 | no rule ledgers a resolved body-less callee `unfetched-dependency` | **fired**: T5 asserts the gap alone; step 6 adds the ledger assertion to T5 |
| F8 | Part 1 SARIF, digest or query differs | a defect: the rule must equal today's; not closed in one attempt → stop |
| F9 | the serializer cannot reach the cone | **fired**: `SnapshotUniverse::CrossApp` carries `rel` (D3) |
| F10 | T4's fixture cannot flag a dependency row `parse_incomplete` (the precondition assert fails) | hand-state it on `A` and `R` in the assembled model before composing; the reachability shape still comes from the source |
| F11 | a lib test reads in-memory gaps of a cross-app model and moves | rewritten in the same commit, named in the plan (I2's exception); a production output move is F8 |
| F12 | the panel moves the exit-code fix (D5.5) out | T6 asserts the diagnostic and empty findings only; T6b dropped; the ledger discovery becomes its own issue |
| F13 | something reads `primary_app_guid` or its census bucket | keep the field, built from the rule |
| F14 | the panel keeps the inventory doc at 1.1.0 | `role`, `analysisUniverse` and the `routineInventory` class still land; T11 asserts 1.1.0 |
| F15 | hand-editing the 12 inputs moves a diff output | the edit is wrong (or preflight is): fix it; outputs must not move |
| F16 | the reach BFS or the reference walk is slow on a large cross-app model | report it in Part 2 (no bound, Decision 58); step 9's peak/time bounds own it |
| F17 | T9-closure finds an id-shaped value under a field not in `REFERENCES` or `NON_REFERENCE_FIELDS` (S9) | classify that field (reference or not) in the same commit, from its serde struct; a field that holds ids is added to `REFERENCES` with its domain (or, if it is a value-source field, to the value-source list) and T9-closure re-run; never fall back to reading every string |
| F18 | T10 finds a cross/single difference (S10) | the parity contract stays as written. Find the cause: if step 8 made it, fix step 8; if it is older (the cut model differs from the single-app one), stop and raise it to the operator with the diff, and file it; T10 is not narrowed |
| F19 | T4c cannot show `Inner` outside the cut's reach (the precondition fails) | the cut includes dependency callers somewhere: find which edge and choose a deeper dependency-only hop (add `Inner → Deeper` to a temp copy of DD's dependency source) |
| F20 | load validation (D5.2) refuses a file some existing test or golden feeds to `diff` | that input is upgraded like D6's (three edits) and named; the rule is not relaxed |
| F21 | the D2 `has_source` assertion fires on a fixture or on CDO | the opaque grouping is wrong for that model: switch loop 2 to key on `dependency_apps[].has_source == false` (the epic's original 8A-5 idea) and add the row to T3; `body-unavailable` then covers the body-less rows of apps with source |
| F22 | a serialized identity's app is missing from `dependency_apps` (the D3 assertion fires) | a model row came from an app the loader did not list as required: stop and find that path; do not emit a guessed `apps[]` row |
| F23 | the case-insensitive shard prefix (O13) moves a shard golden | it cannot on the committed shard goldens (their only guid, `11111111-…-00000000d80a`, is lower case), so a move beyond the allow-list's header and `role` changes is a defect in the prefix change, fixed before merge (§4.4) |
| F24 | the field census finds a field whose values belong to more than one domain (e.g. `subject` holding a routine id or a bare guid) | the domain is decided per VALUE by its shape (bare guid → app; otherwise identity), stated in `REFERENCES` beside that field; never by trying every domain |
| F25 | T9-closure reports a violation in any domain that is not a mapping error: a dangling value the composer already emits today on the base (e.g. a call-site or edge id of an anchorless edge the deriver skips) | prove it is pre-existing by running the same check over the base's in-memory snapshot; record it as a discovery with the value, its key and its domain; add exactly that shape to that domain's explicit external list with a comment naming the discovery; the rule is never loosened for a whole domain |
| F26 | a shard closure adds rows in single-app (a shard golden moves) | a defect, never an allow-list exception: no shard golden may move beyond the header and `role` changes (§4.4). Find the referencing row and the domain; if the slice misses a primary row, fix the closure so it only adds rows the slice could not keep; stop and raise it if the fix would change the shard shape |
| F29 | T10's `Unused` table is named by some field after all (the precondition assert fails) | use another declaration no field names (an enum or page with no code use) and assert the same precondition; the arm and its break are unchanged |
| F30 | a nested value-source arm of T9-closure cannot be hand-stated on a composed DD/R5B model | run that arm on hand-built typed rows through the production filter, as T9g does; the arm and its break are unchanged |
| F31 | the Closure check reports an app-domain violation on a composed (not hand-stated) model: a gap's app guid has no `apps[]` row | treat it like F22: a model row came from an app the loader did not list as required; stop and find that path; never emit a guessed `apps[]` row and never relax the app domain |
| F27 | T12's two targets cannot be built in one process (a global or cache refuses) | build the second from a temp copy of the workspace; the assertion and its break are unchanged |
| F28 | T9-closure cannot find a `table-field` value source in DD or R5B | hand-state one on a kept primary fact (asserted), as the precondition already allows |

## 11. Decisions on the open questions

- **Q1 (8A-2):** keep `dep_routine_ids` as the projection. No reader needs a `RoutineIx`; 71
  reads of mechanical churn buy nothing step 8 delivers.
- **Q2 (8A-3):** `apps[]` = primary + dependency apps owning a serialized identity. Without
  them a cross-app shard of a dependency app has no `apps` row.
- **Q3:** envelope `1.1.0` unchanged (its contract is the wrapper; the payload has its own
  version); inventory `1.1.0` → `1.2.0` (its own payload gains fields).
- **Q4:** reach roots = every primary routine of the model (I4).
- **Q5:** withdrawn by P5 (8A-5).
- **Q6:** the citation is R2d's forbidden list; nothing changes there (8A-6).
- **Q7:** hand-edit the 12 inputs with Edit; a test-side v3→v4 upgrader would be a compatibility
  path the product refuses.
- **Q8:** `"single-app"` / `"cross-app"`.
- **Q9:** the fatal-preflight exit code is fixed here (D5.5), F12 if contested.
- **Q10 (O7):** filter typed rows before CBOR (§4.4), not the finished map.
- **Q11 (S4):** a dependency shard is not a diff input; it is refused at load (D5.2).

Risk: **step 8 changes no command's analysis answer.** Gaps and classes are exercised only by
tests until step 9 wires the commands. The output changes step 8 does make are the schema
header, `apps[].role`, the inventory document, the load refusals and the fatal-preflight exit
code (S17). Step 9's row compares cross-app cli-b with single-app ("contents unchanged") and T10
here covers the no-dependency case.

## 12. What steps 5, 6 and 9 must carry over

- **Step 5 (parallel):** both steps edit `capability_cone.rs` near `build_cross_app_base`
  (step 8: the id-set predicate and `primary_app_guid`; step 5: the uncertainty fold). Text
  conflicts only. Each step checks its OWN golden expectations against its own base (S16):
  step 8's `allowlist.py` runs with `P` = whatever step 8 is rebased on, so step 5's moves are
  already in `P`; step 5 checks its moves by its own row, and if it rebases onto step 8 its `P`
  holds step 8's header. Whichever merges second regenerates cli-b once on the combined tree.
- **Step 6:** (a) its rewrite of `compose_full_snapshot` keeps `opts.universe` and the class
  filter; its scoped composition (`WitnessScope`) must produce exactly the rows this filter
  keeps, and T9/T10 are the oracle (O7). Reach keeps reading `rel.cone()`. (b) T5 gains the
  `unfetched-dependency` ledger assertion (Decision 8's ledger half). (c) T9d gains the
  positive arm "`Inner`'s operation is referenced by `CallChain`'s inherited fact, so it is
  kept" (on the cut no primary row references it). (d) `derive_analysis_gaps` keeps calling the
  rule when its input moves to `caps`.
- **Step 9:** production callers pass `SnapshotUniverse::of(&target)`; cli-b's header flips
  to `"cross-app"`; Decision 28 reads `Role`; `--single-app` on fingerprint, prove, diff. The
  d1 anchor-map discovery (O1) meets cross-app SARIF there.

## 13. Implementation tasks (sizing; the plan comes later)

1. `role.rs`: `Role`, the three `Model` methods, `role_of_stable_id`; T1. Convert every
   converted site of §3.1; delete `primary_app_guid` + census bucket; T1c, T2. CDO identity
   spot-check (SARIF primary) before moving on.
2. Gaps: `derive_analysis_gaps(resolved, base)`, live rule, `role` on `SnapshotAnalysisGap`.
3. Schema 4: one constant; `analysisUniverse`; `SnapshotUniverse` in `FullSnapshotOptions`
   (both production callers pass `SingleApp`); `DependencyApp.publisher`,
   `R3a5CrossAppBase.dependency_apps`; `project_apps` by role; inventory doc 1.2.0, universe in
   payload, `routineInventory` classed.
4. Key classes on typed rows: `KeyClass`, `SNAPSHOT_KEYS`, `Domain`, `REFERENCES`,
   `NON_REFERENCE_FIELDS`, the per-domain resolution, the model-identity assert, the six-step
   order, the value-source walk, `EXTERNAL_IDENTITY_SHAPES`, the `has_source` and
   `dependency_apps` assertions; shard closure to a fixpoint over the identity, operation,
   callsite and edge domains (no frames, no apps) and the case-insensitive prefix; T3, T4, T4c, T5, T8, T9a-g, T9-keys, T9-closure, T10, T12.
5. Preflight: load refusals (schema 3, universe, roles, dependency shard), universe mismatch,
   role-based identity, `DiffEngineResult.fatal` + exit 1; T6, T6b, T7, T7b, T8b.
6. Goldens: hand-edit the 12 diff inputs + PROVENANCE; rewrite the named tests;
   `scripts/check-goldens --regen`; `allowlist.py` with its mutation run; T11; CHANGELOG
   (Changed: schema 4, inventory 1.2.0, fatal preflight exit 1, load refusals; Added:
   `analysisUniverse`, roles; Fixed: shard/preflight role by position, shard reference
   closure).

Rough size: ~16 source files, ~13 test files, 12 hand-edited inputs, 148 regenerated golden
files.
