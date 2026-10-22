# #220 (epic #160 step 8): roles, snapshot schema 4, preflight

Status: spec for the spec panel, round 1. Made from the pre-draft
`/work/bench/160-8/draft-step8-design.md` (written at `5c42b8f3`, unmeasured) once the probes
had run. §0 lists what the probes changed.

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
- **Named** = an id appears as a string value somewhere in a serialized row.
- **Preflight** = the checks `alsem diff` runs before comparing (`gate/diff/preflight.rs`).

## 0. What the probes changed in the pre-draft

1. **F9 fires (P6).** The snapshot is still built on the cut (step 6 has not merged). On CDO,
   22,937 dependency rows are reachable over the cone but only 361 over the cut. Reach must be
   computed from `rel.cone()`, which the caller passes in (D2). The pre-draft took it from step
   6's snapshot graph.
2. **8A-5 is withdrawn (P5).** Body-less dependency rows are exactly the ABI rows of symbol-only
   apps: the cross-app model adds EVERY ABI row of a required symbol-only app and only parsed
   rows of an app with source (`workspace.rs:1600-1630`; `apps_mixed_body` = 0 on CDO and on 4
   fixtures). So today's grouping ("an app owning a body-less dependency row") already is "the
   opaque app". `body-unavailable` stays dead, and Decision 8's gap is the app-level
   `symbol-only-boundary` row. The pre-draft's F6 branch is taken by evidence, not by a panel.
3. **F7 fires.** There is no `unfetched-dependency` ledger status (step 6). T5 asserts the gap
   alone; step 6 adds the ledger half (§10).
4. **P1 found a third role rule:** 11b's `projection::role_of` (`program/model/projection.rs:97-111`)
   reads a row's `source_unit_id` (`ws:` = primary). It agrees with the app-guid rule on every
   CDO and fixture row (P1c). It is kept and pinned by an agreement test (T1c), not replaced
   (§2).
5. **P3, letter falsified, design unchanged.** A single-app CDO snapshot names 5 dependency apps
   outside `inputs`, but only in `eventDeclarations[].eventId` and `.binding.publisherObject`
   (primary subscribers of dependency events). `identities` holds the primary app only, so
   single-app `apps[]` stays the primary row (D3's rule reads `identities`).
6. **CDO has nothing to gap (P5).** Every CDO dependency has source: 0 body-less rows, 0
   parse-incomplete rows, `opaque_apps` 0. The gap filter and the gap half of the key classes
   have no CDO witness. `tests/r2-5b-fixtures/cross-app-resolution` is the witness (2
   symbol-only apps, 8 body-less rows, only app `dddd…0001` reachable).
7. **The other assumptions hold:** the stable-id prefix, the object-id guid, `dep_routine_ids`
   and the registry map all agree with the app-guid rule on CDO (P1b: 31,658 rows and 11,133
   objects, 0 disagreements; P8: 42,625 keys, 0). 11b's role-homogeneity assertion holds on CDO
   (P7: 11b's merge gate ran `cdo-gate` green on this base). The decoder for the allow-list
   reads all 86 cli-b CBOR files equal to their raw JSON (P4b).
8. **New discovery (P9):** a FATAL diff preflight exits 0, even under `--fail-on`
   (`gate/diff/mod.rs:145-153`, `diff/cli.rs:299-307`). A mixed-universe refusal would inherit
   it. Taken in scope (D5.4).

## Feasibility first

| Item | Verdict | Why (tree at `f96619ec`) |
|---|---|---|
| Every cli-b diff checked against an allow-list | **feasible, narrower than it reads** | Only `fingerprint` and `diff` compose the full snapshot (`fingerprint_cli.rs:436`, `gate/diff/cli.rs:140`); both are single-app until step 9. In a single-app model every row is primary (P3), so every key class is the identity: only the header and `apps[].role` move. Every cli-b `analysisGaps` is `[]` (P4), so the gap annotation moves no cli-b byte |
| Live `is_dependency_role` with reachability | **feasible; no production output sees it at step 8** | Stub `snapshot.rs:2433-2435`. Gaps are serialized only by the full snapshot (single-app) and counted by `prove` (single-app, `prove.rs:882`, `:976-984`). `digest` composes a cross-app in-memory snapshot (`digest_cli.rs:1639`) but never reads gaps. Library tests on a cross-app composition carry the proof until step 9 |
| Reachable-only gap test | **feasible, with a committed fixture** | `tests/r2-5b-fixtures/cross-app-resolution`: app `eeee…0002`'s rows are model rows (all ABI rows of a required symbol-only app are added) and none is reachable (P6) |
| Mixed-universe refusal | **feasible** | one more fatal diagnostic beside `app-identity-mismatch` (`preflight.rs:83-101`); plus the exit-code fix (D5.4) so the refusal is visible to CI |
| Shards with a dependency `apps[0]` | **feasible; latent defect today** | `serialize_sharded` takes the primary from `apps[0]` (`snapshot_full.rs:1280`); preflight too (`preflight.rs:25-30`); `project_apps` sorts by guid (`:398`). Position = role only while `apps[]` has one row |
| One test per key class, gaps in both directions | **feasible on a library cross-app composition** | `AnalysisTarget::build(inputs, false, ..)` (`gate/run.rs:179-200`, over the `pub(crate)` `build_cross_app_base`, `capability_cone.rs:3856`) and `compose_full_snapshot` are `pub`; the `dep_dispatch_fixture` (embedded source) and the r2-5b fixture (symbol-only) give every shape |
| CDO identity (Decision 58) | **feasible: the rule is today's rule** | P1b, P1c, P8: 0 disagreements |

## 1. Goal

1. **One role rule.** `Role` is one function of a row's `app_guid` against
   `Model::primary_app`, for routines and objects. It replaces the snapshot stub, the
   registry's two role maps, the base's id set and the other copies of the comparison (§3.1).
   The `dep:` location filter (`registry.rs:544-558`) stays beside it, unchanged.
2. **Live gaps, filtered by reachability when serialized.** `is_dependency_role` becomes
   `role == Dependency`. The in-memory gap table is unfiltered (I8) and every gap row carries
   its `role`. The serialized `analysisGaps` of a cross-app snapshot keeps a dependency gap only
   when its subject is reachable (Decision 46).
3. **Snapshot schema 4.** `schemaVersion: 4`; a header key `analysisUniverse`
   (`"single-app"` | `"cross-app"`); `apps[]` rows carry `role`, primary first; gap rows carry
   `role`. Every top-level key has a class in code; a cross-app serialization applies it.
4. **Preflight.** Schema 3 is refused at load with a remedy. Two snapshots of different
   universes are refused. App identity and shard roles read `role`, never `apps[0]`. A fatal
   preflight exits 1.

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
  that agrees everywhere (P1c). T1c pins the agreement.
- **`body-unavailable` is not revived** (§0.2). The loop stays as it is (dead for every model
  the program builds; harmless).
- **No digest or prove output change**, and no change to the uncertainty split's halves (step
  5's): step 8 swaps only the predicate that builds `dep_routine_ids`.
- **No bench** (Decision 58).

## 3. Background

### 3.1 Where roles are decided today (P1)

| Site | Rule today | After step 8 |
|---|---|---|
| `capability_cone.rs:3866-3880` (`build_cross_app_base`) | `app_guid.to_ascii_lowercase() != primary` → `dep_routine_ids` | built from `routine_role` |
| `capability_cone.rs:3904-3906` | call split on `dep_routine_ids.contains(&e.from)` | unchanged (reads the projection) |
| `capability_cone.rs:3743, 4000, 4035` | `primary_app_guid` field (lowercase) + its census bucket | deleted (its one reader, the registry, calls the rule) |
| `registry.rs:449-456` (single-app) | every routine `"primary"` | `routine_role(r).as_str()` |
| `registry.rs:524-543` (`cross_app_scope`) | routines via `dep_routine_ids`; objects by lowercase compare | `routine_role` / `object_role` |
| `gate/run.rs:244-249` (`is_primary_routine`) | `!dep_routine_ids.contains(&r.id)` | `routine_role(r) == Primary` |
| `gate/run.rs:414-421` (`dependency_object_predicate`) | `!o.app_guid.eq_ignore_ascii_case(p)`; unknown object → `false` | `object_role(o) == Dependency`; unknown object → `false` (kept) |
| `program/model/coverage.rs:243-250` | `is_none_or(eq_ignore_ascii_case)` | `routine_role` |
| `l5/digest.rs:3700-3712` (`reportable_roots`) | `is_some_and(!eq_ignore_ascii_case)` | `routine_role` |
| `l4/effect_query_cli.rs:355-360` | `is_none_or(eq_ignore_ascii_case)` | `routine_role` |
| `l5/selector_index.rs:80-95` | stable id starts with `<guid>:` (case-insensitive) | `role_of_stable_id` (the same test, moved to `role.rs`) |
| `l5/snapshot.rs:2433-2435` | stub `false` | `routine_role(r) == Dependency` |
| `l5/snapshot_full.rs:381-411` (`project_apps`) | primary app only, by guid | D3 |
| `l5/snapshot_full.rs:1280-1286`, `preflight.rs:25-30` | role = position `apps[0]` | the `role` field (D5) |
| `program/model/projection.rs:97-111` (11b `role_of`) | `ws:` unit = primary | kept; T1c |
| `l4/exec_relation.rs:353-359`, `l5/test_support.rs:635` (test helpers) | lowercase compare | `routine_role` |

Every "no primary app" arm agrees today: no primary ⇒ every row primary (the cross-app base
refuses a model without one, `capability_cone.rs:3866-3872`). Two absent-id defaults are
policies for an id the map does not hold, not role rules, and stay: `role_scope_and_sort`
reads an absent routine as primary (`registry.rs:733-736`); `pick_actionable_anchor` as
non-primary (`actionable_anchor.rs:34-35`).

### 3.2 Gaps today (`snapshot.rs:2359-2430`)

Three loops over `base.ws_routines`: `parse-incomplete` per flagged routine;
`symbol-only-boundary` per app guid (`object_id`'s first segment) of a body-less dependency
routine; `body-unavailable` per body-less dependency routine whose app is not in that set (dead,
§0.2). With the stub `false`, loops 2 and 3 emit nothing. `prove` counts a gap whose subject is
a visited routine id, or a bare app guid prefixing a visited stable id (`prove.rs:160-187`): a
per-root reach filter of its own.

### 3.3 The snapshot's top-level keys

Insertion order (`snapshot_full.rs:113-155`, load-bearing for CBOR): `schemaVersion`,
`alsemVersion`, `workspaceFingerprint`, `generatedAt`, `apps`, `identities`, `contractFacts`,
`schemaFacts`, `permissionFacts`, `rootClassifications`, `capabilityFacts`, `typedEdges`,
`operationIndex`, `callsiteIndex`, `callsiteResolutions`, `analysisGaps`, `coverage`, `inputs`,
`eventDeclarations`, then optional `inputsMetadata`, `routineOrderFrames`. The schema version
lives in two constants: `snapshot_full.rs:40` (pub) and `gate/snapshot_deserialize.rs:38`
(private copy).

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
/// snapshot ids and no model rows (selector index, serializer). P1b: the prefix
/// equals the row's app_guid on every CDO row and object.
pub fn role_of_stable_id(primary: Option<&str>, stable_id: &str) -> Role;
```

- A rule, not a store: a role is a function of the row's own `app_guid`, so no `Vec<Role>` is
  kept in sync (8A-4 withdraws the epic §5.5 `rel.role`).
- Every site of §3.1 calls it. `dep_routine_ids` stays as the string-keyed projection, built
  once from `routine_role` where the base builds it, and documented as such (8A-2).
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
  extraction as loop 2).
- **`reach`** = BFS over `rel.cone()` (`TypedEdge.from` → `to`, internal ids) from **every
  primary routine of the model** (Q4; I4: output never depends on which jobs share a view).
  Built once per cross-app serialization. P6: 17 ms on CDO (release-fast, indicative).
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
  pub enum SnapshotUniverse<'a> {
      SingleApp,
      CrossApp { rel: &'a ExecRelation, dependency_apps: &'a [DependencyApp] },
  }
  impl<'a> SnapshotUniverse<'a> {
      /// SingleApp → SingleApp; CrossApp(base) → CrossApp { &base.rel, &base.dependency_apps }.
      pub fn of(target: &'a AnalysisTarget) -> Self;
  }
  ```
  An options field, not a new parameter, so step 6's signature rewrite of
  `compose_full_snapshot` rebases over it without a conflict in the parameter list.
- **`apps[]`** rows: `appGuid, publisher, name, version, role`. Single-app: the primary row
  (today's), `role: "primary"`. Cross-app (8A-3): the primary row first, then every dependency
  app that owns a SERIALIZED `identities` row, sorted by `locale_compare` of guid (today's sort).
  `DependencyApp` (`workspace.rs:1550`) gains `publisher` (from `AppId.publisher`,
  `program_calls.rs:1002-1007`); `R3a5CrossAppBase` keeps `dependency_apps` (cloned before
  `resolved_app_versions` is built from it, `capability_cone.rs:3991-3995`).
- **Versions (Q3):** the capability-snapshot envelope keeps `schemaVersion "1.1.0"` (its payload's
  `snapshotSchemaVersion` carries the change). The routine-inventory document
  (`build_inventory_doc`, `snapshot_full.rs:1137-1233`) copies `apps` from the tree, so its rows
  gain `role`; it also gains `analysisUniverse`, and `INVENTORY_SCHEMA_VERSION` goes
  `"1.1.0"` → `"1.2.0"` (additive, as the 1.0.0 → 1.1.0 bump was, `cli_p1_inventory.rs:91-93`).

### 4.4 D4: key classes in code

```rust
pub enum KeyClass { Header, Input, Inventory, Behaviour, Gap, Evidence }
/// Every top-level key, in insertion order, with its class and (Inventory,
/// Behaviour) the field that names its subject.
pub const SNAPSHOT_KEYS: &[(&str, KeyClass, Option<&str>)] = &[ /* table below */ ];
```

| Class | Keys (subject field) | Serialized rows, cross-app |
|---|---|---|
| Header (8A-1) | `schemaVersion`, `alsemVersion`, `generatedAt`, `analysisUniverse` | as is |
| Input | `workspaceFingerprint`, `inputs`, `inputsMetadata` | as is |
| Inventory | `contractFacts` (`stableId`), `schemaFacts` (`stableId`), `permissionFacts` (`subject`; `permissionSet` when `kind == "declared"`, as `filter_permission_facts`), `rootClassifications` (`routineId`), `eventDeclarations` (`routine`); `apps` (D3) | rows whose subject is primary (`role_of_stable_id`) |
| Behaviour | `capabilityFacts` (`subject`), `coverage` (`subject`), `callsiteResolutions` (`from`), `typedEdges` (`from`), `routineOrderFrames` (map key) | rows whose subject is primary |
| Gap | `analysisGaps` | D2 |
| Evidence | `operationIndex` (`operationId`), `callsiteIndex` (`callsiteId`), `identities` | rows NAMED by a kept Inventory/Behaviour/Gap row; `identities` = every primary id plus every named id (Decision 17) |

- **One filter function,** `apply_key_classes(map, universe, model)`, runs at the end of
  `compose_full_snapshot`. In `SingleApp` it returns at once (every row is primary, P3), so the
  single-app path is today's plus the header and `role` fields.
- **"Named"** is computed generically: every `Text` value of every kept Inventory, Behaviour and
  Gap row, plus each value with `/` replaced by `:` (facts keep INTERNAL resource ids,
  `snapshot.rs:1403-1406`; the identity table holds stable ids). Then the kept Evidence rows'
  `Text` values join the set before `identities` is filtered (an operation row names its
  routine). Generic on purpose: a field added later is covered without a code change. An id
  can only be named if it is a string in a kept row; ids of different classes never share a
  shape, so no row is kept by accident.
- **Sharding reuses the field table.** `slice_for_app` (`snapshot_full.rs:1318-1376`) keeps its
  own "this app's rows" predicate (prefix `<guid>:`); only its key list moves to read
  `SNAPSHOT_KEYS`, so a key without a class fails there too.
- **Key-set equality:** a test composes both universes and asserts the map's keys ⊆
  `SNAPSHOT_KEYS` and every non-optional key present (T9-keys). The in-memory
  `CapabilitySnapshot` stays unfiltered (I8).

### 4.5 D5: preflight, shards, exit code

1. **Schema 3 refused at load.** `deserialize_snapshot` already refuses any version but the
   constant. The message gains the remedy: `deserializeSnapshot: snapshot schemaVersion 3 is not
   supported by this build (v4); re-create it with 'alsem fingerprint'`. A v4 file without
   `analysisUniverse`, or with another value, is refused the same way (exit 1, as P9 measured).
2. **Universe mismatch.** `run_preflight` adds a fatal `analysis-universe-mismatch`
   (`kind`, `old`, `new`), checked right after `schema-version-mismatch`, same shape as
   `app-identity-mismatch`.
3. **Identity by role.** `primary_app_id` reads the `apps[]` row with `role == "primary"`;
   `serialize_sharded` reads each row's `role` (`primary-only` keeps `role == "primary"`).
   `apps[0]` is never read for a role. The manifest shape is unchanged.
4. **A fatal preflight exits 1 (decision, P9).** `DiffEngineResult` gains `fatal: bool`;
   `diff/cli.rs`'s exit gate sets 1 when it is true, before the coverage and `--fail-on`
   gates. Stated behaviour change: `app-identity-mismatch`, `schema-version-mismatch` and
   strict `alsem-version-mismatch` also exit 1 now (they exited 0). stdout is unchanged. Why in
   step 8: E8's "refused" means nothing to CI if the refusal exits 0, and the universe check is
   the one step 9 makes reachable from real files.

### 4.6 D6: the vendored diff inputs

The 12 `tests/cli-b-goldens/diff/snapshots/*.snap.json` are INPUTS at schema 3, "never
regenerated" (`diff/PROVENANCE.md`). Q7: they are upgraded by hand with Edit, three edits each
(`schemaVersion` 4; `"analysisUniverse": "single-app"`; `"role": "primary"` on the one `apps[]`
row), and PROVENANCE gains one paragraph saying so. Every diff OUTPUT must stay byte-identical.
The inline snapshots of `cli_b_diff_differential.rs` (`snapshot()`, `:489-500`) use the
constant, the universe key and a `role` on their app rows; the schema-mismatch oracle
(`:545-566`) keeps its 99 sentinel.

### 4.7 What is deleted

The stub, the private schema constant, the lowercase/`eq_ignore_ascii_case` copies of §3.1,
`R3a5CrossAppBase::primary_app_guid` and its census bucket `base.primary_app_guid` (no test
reads it; stderr census only, stated). Fallback F13 if something does read it.

## 5. Test plan (every new test: break, failing assertion, revert; I9)

Fixtures: **DD** = `tests/common/dep_dispatch_fixture.rs` (`write_own`; embedded-source
dependency `XBase`, workspace guid `aaaa…` sorts BEFORE `bbbb…`). **R5B** =
`tests/r2-5b-fixtures/cross-app-resolution` (two symbol-only apps; `dddd…0001` reachable,
`eeee…0002` not; P6). Cross-app models come from `AnalysisTarget::build(inputs, false, ..)` and
the composition from `compose_full_snapshot` with `SnapshotUniverse::of(&target)`. New
integration tests live in a new `tests/cli/cli_snapshot_schema4.rs` member; lib tests in
`role.rs`, `projection.rs` (T1c: `role_of` is private) and `snapshot_full.rs`.

| # | Test | Precondition (asserted) | Break → fails |
|---|---|---|---|
| T1 | `role_rule_hand_stated` (lib): primary guid upper and lower case, a dependency guid, `primary_app: None`; `role_of_stable_id` with `<PRIMARY>:x`, `<dep>:x`, `<primary>x` (no colon) | rows built by assignment | case-sensitive compare → upper-case row `Dependency`; drop the `None` arm → `Dependency`; drop the `:` check → `<primary>x` reads `Primary` |
| T1c | `projection_role_of_agrees_with_rule` (lib, `projection.rs`; R5B read from `CARGO_MANIFEST_DIR`, plus an embedded-source dependency written to a temp dir as 11b's tests do) | both kinds of dependency row present (symbol-only, `dep:` source) | for every row, `role_of(r).is_none() == (routine_role(r) == Primary)`; break: make `role_of` treat `dep:` as primary → fails |
| T2 | `cross_app_scope_uses_role` (lib): a dependency-object-anchored finding and a dependency-routine-anchored one are dropped under `--scope primary`, kept under `--scope all`; a `dep:`-unit finding with a primary actionable anchor is kept | the model holds both dependency rows | `role_of_app` → always `Primary` → both kept, fails. The existing `dep:` test stays |
| T3 | `gaps_reachable_only` (R5B, cross-app full snapshot) | in memory: `symbol-only-boundary` for BOTH apps (each with `role: "dependency"`); `eeee…` rows are model rows and none is in `reach` | serialized: `dddd…` present, `eeee…` absent. Break 1: skip the reach filter → `eeee…` appears. Break 2: stub back to `false` → `dddd…` missing. Both directions of Decision 46 |
| T4 | `parse_incomplete_gap_filter` (temp fixture: embedded-source dependency with a reachable routine `A` and a reverse-demand raiser `R` (raises a dependency event a primary routine subscribes to), both with a syntax error; a primary routine with one too) | `R` is a model row not in `reach`; all three flagged | serialized: primary and `A` gapped, `R` not. Break: filter primary rows too → primary gap missing; break: no filter → `R` appears. F10 if the fixture cannot flag a dependency row |
| T5 | `symbol_only_callee_has_its_gap` (R5B) | a primary routine's typed edge targets a `dddd…` body-less row (asserted on `rel.cone()`) | the serialized `symbol-only-boundary` for `dddd…` with `role: "dependency"`. Break: drop the cone edge from reach (BFS over the cut) → the gap disappears. Step 6 adds: the call's ledger row is `unfetched-dependency` (F7) |
| T6 | `diff_refuses_mixed_universes` (cli): two v4 files, same app, `single-app` vs `cross-app` | both load | `analysis-universe-mismatch` in the diagnostics, no findings, exit 1. Break: drop the check → findings computed, exit 0 |
| T6b | `fatal_preflight_exits_one` (cli): two vendored inputs of different apps | both load | exit 1 with `app-identity-mismatch`. Break: drop the `fatal` gate → exit 0 (P9's measured behaviour) |
| T7 | `diff_refuses_schema_3` (cli): a temp copy of a vendored input edited back to `schemaVersion: 3`, diffed against a v4 file | the copy carries 3 (asserted) | stderr names 3, v4 and the remedy; exit 1. Break: accept 3 → loads, fails |
| T8 | `shards_read_role_not_position` (lib): a hand-built v4 tree whose `apps[]` lists a dependency (`0000…`) before the primary | hand-built, not composed (the composer cannot produce this order) | manifest roles right; `primary-only` writes the primary's shard only; `primary_app_id` = the primary guid. Break: read `apps[0]` → fails |
| T9a Inventory | DD cross-app | in memory, `contractFacts`/`schemaFacts`/`rootClassifications`/`eventDeclarations` hold `Dep Lib`/`Dep Sub` rows | serialized: none. Break: identity filter → they appear |
| T9b Behaviour | DD cross-app | in memory, `Inner` has direct capability facts and coverage | serialized: no `capabilityFacts`/`coverage`/`typedEdges`/`callsiteResolutions` row with a dependency subject; `CallChain`'s rows present. Break: identity filter → `Inner` appears |
| T9c Gap | T3 and T4 | — | — |
| T9d Evidence | DD + R5B cross-app | `Outer` is named by `CallChain`'s typed edge; R5B's `eeee…` rows are unnamed | `identities` holds `Outer` and every primary id, and no `eeee…` id. Break 1: emit every row → `eeee…` appears. Break 2: primary only → `Outer` missing (dangling `to`) |
| T9e Input | DD single vs cross | — | `workspaceFingerprint`, `inputs` equal across universes. Break: filter `inputs` as Inventory → dependency `.app` rows vanish |
| T9f Header | DD both universes | — | `analysisUniverse` per universe; `apps[0].role == "primary"`; cross-app `apps` holds `bbbb…` with `role: "dependency"`, publisher `probe`. Break: sort apps by guid only → still `aaaa…` first on DD, so the test ALSO composes R5B-like order via T8 (stated limit) |
| T9-keys | both universes, DD | — | composed keys ⊆ `SNAPSHOT_KEYS`, required keys present. Break: insert a new key without a class → fails |
| T10 | `no_dependency_cross_equals_single` (`tests/r0-corpus/ws-d8-commit-in-tx`) | the cross-app base builds with no dependency | full trees equal except `analysisUniverse`. Break: add `role` to gap rows only in cross-app → fails. F18 if a pre-existing difference shows |
| T11 | `cli_p1_inventory` (edited) | — | inventory doc `schemaVersion "1.2.0"`, `apps[0].role == "primary"`, `analysisUniverse == "single-app"`; the projection-subset check (`:105-125`) still byte-equal |

Tests rewritten with the code they pin (I2's exception): `snapshot_deserialize.rs`
`deserialize_json_raw`, `deserialize_json_enveloped`, `normal_gz_snapshot_round_trips_under_cap`,
`roundtrip_*` (they build `Int(3)` trees; now the constant + `analysisUniverse`);
`wrong_schema_version_errors` gains 3 in its refused list; `cli_b_diff_differential.rs`
`snapshot()` and `oracle_preflight_app_identity_mismatch_is_fatal` (its app rows gain `role`).
Any other lib test that reads in-memory gaps of a cross-app model is named in the plan (F11).

## 6. Acceptance matrix

| # | Issue / epic item | Evidence |
|---|---|---|
| A8.1 | `Role` with the `dep:` filter beside it | D1; T1, T1c, T2; `rg 'eq_ignore_ascii_case\(p\)\|to_ascii_lowercase\(\) *!= *primary' src` hits only `role.rs` (test helpers included) |
| A8.2 | Live `is_dependency_role` with reachability | D2; T3, T4, T5 |
| A8.3 | Key classes | D4; T9a-f, T9-keys |
| A8.4 | Snapshot schema 4 | D3; `rg 'SNAPSHOT_SCHEMA_VERSION: i64' src` = 1 hit |
| A8.5 | `apps[]` by role | D3, D5.3; T8, T9f |
| A8.6 | Preflight | D5; T6, T6b, T7, T8 |
| A8.7 | Every cli-b diff checked against an allow-list (header, `apps[]` roles, gap role annotation; anything else fails) | §8 Part 0: `allowlist.py` base vs head over all of `tests/cli-b-goldens`, fails closed; the 12 diff inputs as D6's three edits; every diff OUTPUT byte-identical |
| A8.8 | Tests: reachable-only gap; mixed-universe refusal; shards with a dependency `apps[0]`; one test per key class, gaps in both directions | T3 (+T4, T5); T6 (+T6b, T7); T8; T9a-f (gaps both directions = T3's two breaks) |
| A8.9 | Golden "every cli-b file" | **narrowed, with the reason.** Moves: the 62 schema-3 JSON files (P4) and the 86 CBOR files. Does NOT move: digest, prove, fingerprint-query JSON, every `.human.txt`, every diff output (none carries the header, `apps` or gaps, P4). Part 0 fails if any of those changes |
| A8.10 | Golden `cli_p1_inventory` | T11 (test code: version 1.2.0, `role`, `analysisUniverse`) |
| A8.11 | Golden `cli_workspace_fingerprint_closure` | **no change expected**: it reads `workspaceFingerprint`, `inputs` and digest's dependency list (`:37-80`); the fingerprint hashes inputs only (`snapshot_full.rs:894-908`). It must pass unchanged; a failure is a defect (F5) |
| A8.12 | "the key list in `tests/differential.rs` (~1758-1799)" | **not deliverable as written, because** those lines are R2d's `L3COV_FORBIDDEN_KEYS` (`:1781`), the L3 coverage surface's forbidden keys, not a snapshot key list (P10). `analysisUniverse` never enters that surface, so nothing there changes. The guard the citation aims at ("every snapshot key has a class") is T9-keys over `SNAPSHOT_KEYS` (8A-6) |
| A8.13 | Non-speed (Decision 58): no bench | §8: `cdo-gate` + identity only |
| E6 (8's half) | cross-app prove never falsely PROVEN | T3, T5: the reachable opaque boundary is a gap row with a bare app guid, which `count_gaps_in_cone` matches (`prove.rs:174-187`); step 9 wires prove |
| E7 (8's half) | every key classed | T9-keys |
| E8 | mixed universes / schemas refused | T6, T6b, T7 |
| E14 / I10 | CDO ratchets | `scripts/cdo-gate` green |
| I2 (scoped) | what stays byte-identical | the 7 golden targets other than `--test cli`; in `--test cli`, every family except the cli-b files of A8.9 and `cli_p1_inventory`'s assertions; `cargo test -p al-sem --lib` outcomes apart from §5's named rewrites; CDO SARIF (4 runs) and histogram SHA |
| I4 | output independent of the job set | reach roots = every primary routine (D2) |
| I8 | serialized inventory primary-only, in-memory unfiltered | D2, D4; T9a; the in-memory precondition asserts of T3/T9a |
| I9 | discrimination | every row of §5 |

Untouched: I1 (no build counter moves; the BFS is not one), I3, I5, I6, I7, I11.

## 7. False-positive and failure shapes

- **A site whose rule differs from today's:** P1b/P1c/P8 show none on CDO; Part 1's SARIF and
  digest identity catch a slip (F8).
- **A collided string id with two roles:** 11b's assertion (`projection.rs:48`), green on CDO (P7).
- **Demand churn as a false gap or false behaviour row:** T3's negative arm; T9a/b.
- **A gap hidden by over-filtering (later a false PROVEN):** T3's positive arm, T5.
- **Reach computed over the cut instead of the cone:** T5's break (on CDO the cut reaches 361
  of 22,937 rows, P6).
- **Reach from the job's subjects:** D2's roots take no job input at all (the composition has
  no subject parameter at step 8), so this shape cannot arise yet; step 12's batch contract
  test is where a job-dependent root would show.
- **A dangling Evidence reference:** T9d's second break.
- **Position read as role:** T8.
- **A key added later without a class:** T9-keys.
- **A silent mixed-universe or stale-schema diff:** T6, T6b, T7.
- **The CDO run proves nothing about gaps:** stated limit (§0.6); the fixtures carry it.

## 8. Measurement plan (Decision 58: non-speed, no bench)

Run once after the final panel by the conductor. `P` = branch base, `H` = head. `WS` = the
pinned CDO workspace (`.agent/config.json`), never written into a file.

- **Part 0, the cli-b allow-list (repo).** After `scripts/check-goldens --regen` on `H`:
  `python3 -I /work/bench/160-8/allowlist.py <P-tree>/tests/cli-b-goldens <H-tree>/tests/cli-b-goldens`
  exits 0. It decodes CBOR with `cbordec.py` (P4b) and parses JSON; it removes exactly these
  paths from both sides and requires the rest equal: `schemaVersion`, `snapshotSchemaVersion`,
  `analysisUniverse`, `apps[*].role`, `analysisGaps[*].role`, the manifest's `schemaVersion`.
  It exits 2 on a missing or unparseable file, on a changed file outside A8.9's list, and on any
  other difference. The 12 diff inputs are checked as D6's three edits (`H` minus those three
  paths equals `P`).
- **Part 1, identity (CDO).** `alsem analyze <WS> --format sarif` at `--scope primary` and
  `--scope all`, default (cross-app) and `--single-app`: `P` vs `H` byte-identical (4 pairs).
  `aldump --program-call-graph-stats <WS>` SHA equal. `alsem digest <WS> --routine <a primary
  root> --format json --deterministic` `P` vs `H` byte-identical (the cross-app in-memory
  snapshot now carries live roles; nothing it outputs may move). `bash scripts/cdo-gate` green.
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
  `body-unavailable` row arises in any model the program builds.
- **8A-6** Row 8's golden "the key list at `tests/differential.rs:1758-1799`" is withdrawn
  (P10, A8.12); T9-keys is the snapshot key-list guard.

## 10. Fallbacks (every "if X", pre-stated)

| # | If | Then (no new panel round unless it moves output) |
|---|---|---|
| F1 | P1b disagreed | **not fired** (0). Had it: the selector index calls the rule via the identity row |
| F2 | 11b's homogeneity assertion fired on CDO | **not fired** (P7). Had it: stop, a per-id role is a merge |
| F3 | a single-app snapshot names dependency apps | **fired in letter** (eventDeclarations, P3); `apps[]` reads `identities`, which is primary-only single-app, so nothing moves |
| F4 | converting a site needs edits in > 40 files | **not fired** (14 files, not re-keyed) |
| F5 | `allowlist.py` finds a non-allowed difference, or `cli_workspace_fingerprint_closure` fails | a defect; nothing merges until it is explained in source |
| F6 | 8A-5 contested | **taken by evidence** (P5) |
| F7 | no `unfetched-dependency` ledger status | **fired**: T5 asserts the gap alone; step 6 adds the ledger assertion to T5 |
| F8 | Part 1 SARIF or digest differs | a defect: the rule must equal today's; not closed in one attempt → stop |
| F9 | the serializer cannot reach the cone | **fired**: `SnapshotUniverse::CrossApp { rel, .. }` carries it (D3) |
| F10 | T4's fixture cannot flag a dependency row `parse_incomplete` (the precondition assert fails) | hand-state it: set `parse_incomplete = true` on `A` and `R` in the assembled model before composing (CLAUDE.md "hand-state the precondition"); the reachability shape still comes from the source |
| F11 | a lib test reads in-memory gaps of a cross-app model and moves | rewritten in the same commit, named in the plan (I2's exception); a production output move is F8 |
| F12 | the panel moves the exit-code fix (D5.4) out | T6 asserts the diagnostic and empty findings only; T6b dropped; the ledger discovery becomes its own issue |
| F13 | something reads `primary_app_guid` or its census bucket | keep the field, built from the rule |
| F14 | the panel keeps the inventory doc at 1.1.0 | `role` and `analysisUniverse` still added; T11 asserts 1.1.0 |
| F15 | hand-editing the 12 inputs moves a diff output | the edit is wrong (or preflight is): fix it; outputs must not move |
| F16 | T9f's DD order cannot discriminate primary-first (primary sorts first by guid anyway) | T8 carries the order break; stated limit of T9f |
| F17 | "named" keeps a dependency id through a non-id string equal to it | cannot happen by id shape (`<guid>:` / `r0/<hex>/…`); if a test shows it, restrict "named" to the fields T9d lists |
| F18 | T10 finds a cross/single difference not caused by step 8 | record it as a discovery; narrow T10 to "the class filter is the identity on that model" |

## 11. Decisions on the pre-draft's open questions

- **Q1 (8A-2):** keep `dep_routine_ids` as the projection. No reader needs a `RoutineIx`; 71
  reads of mechanical churn buy nothing step 8 delivers.
- **Q2 (8A-3):** `apps[]` = primary + dependency apps owning a serialized identity. Without
  them a cross-app shard of a dependency app has no `apps` row and "shards with a dependency
  `apps[0]`" tests nothing reachable.
- **Q3:** envelope `1.1.0` unchanged (its contract is the wrapper; the payload has its own
  version); inventory `1.1.0` → `1.2.0` (its own payload gains fields).
- **Q4:** reach roots = every primary routine of the model (I4).
- **Q5:** withdrawn by P5 (8A-5).
- **Q6:** the citation is R2d's forbidden list; nothing changes there (8A-6).
- **Q7:** hand-edit the 12 inputs with Edit; a test-side v3→v4 upgrader would be a compatibility
  path the product refuses.
- **Q8:** `"single-app"` / `"cross-app"`.
- **New, Q9:** the fatal-preflight exit code is fixed here (D5.4), F12 if contested.

Risk: **step 8 changes no production answer.** Gaps and classes are exercised only by tests until
step 9 wires the commands. Step 9's row compares cross-app cli-b with single-app ("contents
unchanged") and T10 here covers the no-dependency case.

## 12. What steps 5, 6 and 9 must carry over

- **Step 5 (parallel):** both steps edit `capability_cone.rs` near `build_cross_app_base`
  (step 8: the id-set predicate and `primary_app_guid`; step 5: the uncertainty fold). Text
  conflicts only. Whichever merges second regenerates cli-b: step 5 moves digest/prove (and
  maybe snapshot `coverage`), step 8 moves snapshot/fingerprint/shard headers, roles and the
  12 inputs. The second runs `allowlist.py` against the first's merge as `P`.
- **Step 6:** (a) its rewrite of `compose_full_snapshot` keeps `opts.universe` and calls
  `apply_key_classes` on whatever tree it composes; reach keeps reading `rel.cone()` (the same
  graph step 6 moves the snapshot onto). (b) T5 gains the `unfetched-dependency` ledger
  assertion (Decision 8's ledger half). (c) T9d gains the positive arm "`Inner` is named by
  `CallChain`'s inherited fact" (on the cut, `Outer → Inner` is a dependency caller's call, so
  no primary row names `Inner` yet). (d) `derive_analysis_gaps` keeps calling the rule when its
  input moves to `caps`.
- **Step 9:** production callers pass `SnapshotUniverse::of(&target)`; cli-b's header flips
  to `"cross-app"`; Decision 28 reads `Role`; `--single-app` on fingerprint, prove, diff.

## 13. Implementation tasks (sizing; the plan comes later)

1. `role.rs`: `Role`, the three `Model` methods, `role_of_stable_id`; T1. Convert every §3.1
   site (base id set, registry, gate/run, coverage, digest, effect query, selector index, test
   helpers); delete `primary_app_guid` + census bucket; T1c, T2. CDO identity spot-check
   (SARIF primary) before moving on.
2. Gaps: `derive_analysis_gaps(resolved, base)`, live rule, `role` on `SnapshotAnalysisGap`.
3. Schema 4: one constant; `analysisUniverse`; `SnapshotUniverse` in `FullSnapshotOptions`
   (both production callers pass `SingleApp`); `DependencyApp.publisher`,
   `R3a5CrossAppBase.dependency_apps`; `project_apps` by role; inventory doc 1.2.0 + universe.
4. Key classes: `KeyClass`, `SNAPSHOT_KEYS`, `apply_key_classes` with reach and "named";
   `slice_for_app` reads the table; T3, T4, T5, T9a-f, T9-keys, T10.
5. Preflight and shards: load refusal + remedy, `analysis-universe-mismatch`, role-based
   identity and shards, `DiffEngineResult.fatal` + exit 1; T6, T6b, T7, T8.
6. Goldens: hand-edit the 12 diff inputs + PROVENANCE; rewrite the named tests; `scripts/check-goldens
   --regen`; `allowlist.py` (written in `/work/bench/160-8/`); T11; CHANGELOG (Changed:
   schema 4, inventory 1.2.0, fatal preflight exit 1; Added: `analysisUniverse`, roles; Fixed:
   shard/preflight role by position).

Rough size: ~15 source files, ~12 test files, 12 hand-edited inputs, 148 regenerated golden
files.
