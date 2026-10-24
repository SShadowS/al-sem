# Issue #264 — interface dispatch closed over a non-extensible enum

Status: design, revision 3 (round 1: sol "rework", opus "approve with
fixes"; round 2: opus approve, sol approve with fixes S1/S8; coordinator
decision after round 2: ABI `Values` ingestion is deferred). Base: `master` 9bd2996f. Probe artifacts:
`/work/bench/264/` (alc probes in `probes/`, CDO runs in `cdo/`, repro runs
`repro-*.master.json`). Every path below is relative to the worktree
`/work/al-sem-issue-264-a1` unless absolute.

## Goal

When an interface variable's value provably comes from an enum that is NOT
extensible, the call `Strategy.NextKm(...)` is a CLOSED dispatch: no other app
can add a value, so no other app can add an implementation. Today every
interface site is open-world, so CentralGauge's #256 reproduction with
`Extensible = false` still gives d35 an `interface-open-world` "unknown"
finding. After this change:

- the site's edge is `SetCompleteness::Complete`;
- the model marks the site closed (`DispatchMeta::closed_over`), the detector
  layer emits no `interface-open-world` uncertainty for it, the snapshot row
  says `openWorld: false`, and `prove`/`digest` do not turn it into a blocker;
- d35 sees a complete cone and reports nothing when no candidate commits.

"Provably" is the whole design. A closed claim is an ABSENCE proof (no other
value, no other write, no other implementation), so every step below fails
OPEN on any doubt.

## Assumption probes

| probe | result | evidence |
|---|---|---|
| P1a resolver | The receiver is typed from its declared type only. `src/program/resolve/resolver.rs:2930-2992` fans out over `index.implementers_of` (codeunits only, built at `src/program/resolve/index.rs:598-611`, read at `:879`) as `Polymorphic`. `src/program/resolve/full.rs:438-443` maps `Polymorphic` to `Partial{ReverseDependentImplementers}` unconditionally; applied at `full.rs:736`. `full.rs:670-672` records `SiteFacts.interface`. | read; repro switch dump `/work/bench/256/repro-closed/switch/calls.edges.txt`: `Maybe`, `to: Demo Impl.NextKm`, `total_impls 1`, `enum_implementers [Enum 50102]` |
| P1b north-star bucket | A site with a real route already counts `resolvedSource`/`resolvedAbiExternal`: `src/program/resolve/edge.rs:640-660` returns before completeness is read. Completeness matters only for an EMPTY fan-out (`edge.rs:667-677`: open+empty → `HonestEmpty`; closed+empty → `Unknown`). `conditionalResolved` here means "every route needs a manual `BindSubscription`". Only `edge.rs:669` and `src/program/graphify_export.rs:500` read completeness (opus verified). | repro stats: `resolvedSource 4`, `conditionalResolved 0`; CDO `cdo/stats.json`: whole `conditionalResolved 266` (events), primary `0` |
| P1c adapter | `src/program/model/program_calls.rs:2023-2150`: one `Maybe` edge per model implementer, one to-less `ExternalTarget` edge per implementer outside the model (`:2060-2070`, `:2134-2147`); `DispatchMeta` only on `out[0]` (`:2148`). | read |
| P1d uncertainty | `src/engine/l4/combined_graph.rs:358-381` emits one `interface-open-world` per targeted interface call site; `:384-404` the same for a to-less interface edge. Site ownership rules at `:310-327`. | read |
| P1e snapshot / ledger readers | `src/engine/l5/snapshot.rs:2201` sets `open_world: Some(true)` for every interface group, reading `representative.dispatch_meta` (`:2184`). `src/engine/l5/test_coverage.rs:502-515` has its own closed rule. `src/engine/l5/unresolved_cone.rs:147-149` harvests every row that is not `resolved`/`builtin` (so `polymorphic` rows) and keeps `openWorld` only when `Some(true)` (`:178`); `src/engine/l5/prove.rs:199-205,233-251` counts a row without `openWorld` as an `unresolved-callsite` blocker; `src/engine/l5/digest_cli.rs:1148` emits `openWorld` only when true. **So `openWorld: false` alone would make a closed site look WORSE in prove/digest** (opus 1, confirmed). | read |
| P1f d35 path | The kind is call-site-local (`src/engine/l4/db_effect_solver.rs:842-847`); `src/engine/l4/capability_cone.rs:3568-3603,4327-4345` turn it into a coverage reason and drop `complete`→`partial`; `src/engine/l5/capability_query.rs:152-166` answers `Unknown`; `src/engine/l5/detectors/d35.rs:62-75,149-170` reports it at medium. `db_effect_solver.rs:1071` adds an `opaque-callee` uncertainty for every `interface` edge; that kind is NOT a coverage reason, so it caps confidence but does not make the cone partial. `src/engine/l5/confidence.rs:112` maps `interface-open-world` to the `dynamic-dispatch` cap. | read; master run `/work/bench/264/repro-closed.master.json`: medium d35, reason `interface-open-world` (also `repro-direct-closed`, `repro-ext`) |
| P2a Extensible default | **Absent = not extensible.** An enumextension of an enum without `Extensible` fails AL0504, like `Extensible = false`; `true` compiles. Symbol files hold `"1"`, `"0"`, or no entry. | alc 18.0.41.62505, `probes/ext-{absent,false,true}`: `error AL0504: The enum 'P Kind' is not extensible`; `probes/sym-absent/out.app`: no `Extensible` property; `ext-true`: `"Value":"1"`; Base App `"0"` ×47, `"1"` ×498, absent ×21 |
| P2b another app | Not compiled cross-app; the same-app rejection (AL0504) is the evidence, and a dependent app compiles against the same symbol. | as P2a |
| P2c value → implementation | Each value must map to an implementation of each implemented interface, or the enum declares `DefaultImplementation` for it (AL0596 otherwise). `UnknownValueImplementation` serves an ordinal that matches no value (`FromInteger(7)`); without it that is a runtime error, not a call. Symbol shape: value `Implementation: "50101"`, multi-interface `"7031,7012"` positional over `ImplementedInterfaces`; `DefaultImplementation`/`UnknownValueImplementation` are object properties with codeunit ids. Source shape: value property `Implementation = "I" = "Impl"[, "J" = "Impl2"]`; object properties `DefaultImplementation = "I" = "Impl"`. | `probes/impl-missing`: AL0596; `probes/impl-default` compiles and its symbol holds `DefaultImplementation 50104`, `UnknownValueImplementation 50105`; Base App `Price Source Type`; [MS Learn, DefaultImplementation property](https://learn.microsoft.com/en-us/dynamics365/business-central/dev-itpro/developer/properties/devenv-defaultimplementation-property) |
| P2d data flow | The variable must be a routine local; every write must come from a closed enum; it must never be written any other way. Flow order does not matter (if every write is closed, every value is). An unassigned local raises a runtime error and calls nothing. | AL semantics |
| P2e `(V)` by reference | A parenthesized variable cannot be passed to a `var` parameter: `Swap((S))` with `procedure Swap(var X: Interface ...)` fails **AL0130** "A 'var' argument must be an assignable variable". So sol 6's `Replace((V))` cannot reassign V. The design still declines every argument occurrence (the allowlist, §3), so the point is moot for soundness; the test pins the decline. | `probes/paren-var` (alc 18.0.41.62505) |
| P3 existing typing | `infer_receiver_type_for_expr` (`src/program/resolve/receiver.rs:1443`) yields `EnumType{name_lc}` — a NAME, not an identity. A record field's type text is read from the field (`receiver.rs:1994-2009`, `index.field_in_table` at `index.rs:924`, which returns the `FieldNode` without its declaring object) and re-resolved in the CALLER's context (`parsed_type_to_receiver`, `receiver.rs:3177-3211`), where an own-app object of the same name wins (`src/program/graph.rs:90-138`). **So a name cannot prove which enum a value belongs to** (sol 1, confirmed). The `X::Value` arm drops the name for `Enum::"K"::V`/`EnumVar::V` (`receiver.rs:1612-1616`); `"K"::V` keeps it via `:1628-1641`. `FromInteger` on `EnumTypeStatic` types as `Unknown` (`receiver.rs:1954-1962`). Only a first-assignment walker exists (`src/program/body/ir_walk.rs:2402-2470`), and it matches `ExprKind::Identifier` only (`:2440-2441`). No extensibility or value-mapping fact exists: `ObjectNode` (`src/program/node_extract.rs:127-183`), `AbiObject` (`src/engine/deps/symbol_reference.rs:264-290`, `derive(Default)`, built with `..Default::default()` in `src/engine/deps/projection.rs:446`), `RawObject` (`symbol_reference.rs:494-513`, no `Values`). Enum values (`enum_value_declaration`) are not lowered (`crates/al-syntax/src/ir/decl.rs:9-58`; one `ObjectDecl` construction, `crates/al-syntax/src/lower/mod.rs:335`). Object properties ARE lowered for every object body (`lower/mod.rs:320-327`, `collect_properties` unions `#if` arms, `:791-800`). | read, grep |
| P3b dedup and recovery | Two whole-object `#if` alternatives give two `ObjectNode`s with one id; `src/program/build.rs:307-308` (dependency sources) and `:370-371` (workspace) keep the first (sol 2, confirmed). `src/snapshot/parse.rs:39-58`: a `Recovered` file cannot back an absence proof. `ObjectNode::parse_incomplete` (`node_extract.rs:160-182`) is the file-level flag. `build.rs:260-281`: ABI ingestion continues after a read failure (`abi_ingest_errors`). | read |
| P3c pack | `ObjectNode` is serialized in dependency packs; `src/program/pack/mod.rs:55-71`: a payload-shape change bumps `PACK_SCHEMA` (now 6). | read |
| P4a CDO workspace | 19 interface call sites (30 edges: 25 `Maybe`, 5 `ExternalTarget`) in the single-app switch-dump model. 5 source enums implement interfaces, all `Extensible = true`. 2 sites are enum-sourced from a closed enum: `CDOeDocPrePostValid.Codeunit.al:28-29,54-55` (`IPrePostValidator := GetPrePostValidationType()`, own-object procedure returning `Enum "CTS-CDN PrePost Valid. Type"`, CDN enum 6086237, `Extensible "0"`, values map to 6225611/6225586 = its 2 implementers). In the single-app model both sites are 2× to-less `ExternalTarget` with `unresolved_impls []` (`cdo/switch/calls.edges.txt:3085-3093`) → **0 closed there** under §5's rule. In the default cross-app `analyze` model the CDN bodies are demanded, so the edges carry targets → **2 closed predicted** (not yet measured). | `cdo/switch/calls.edges.txt` (release-fast master); source grep |
| P4b dependency population | 12 interface-implementing enums in the 11 `.alpackages` are not extensible (5 Continia, 3 Base App, 4 System App; 2 with `Extensible` absent). Their source has ~55 assignments from them: `ICDataExchange := ICPartner."Data Exchange Type"` ×12, `IDimPerspective := DimPerspectiveName."Perspective Type"` ×8, `Enum::SignatureAlgorithm::RSA` ×3, `FromInteger(Ordinal)` ×5, enum locals/params. Regex estimate; M2 gives the engine number. | symbol + source extraction under the session scratchpad |
| P4c d35 on CDO | primary: 52 d35 (11 high, 41 medium), 41 name `interface-open-world`, 6 as the only reason; all: 162 (79/83), 81, 19. Every sole-reason finding's "partly analysed" routine dispatches over an extensible or list-held interface (`Telemetry Logger` from a `List of [Interface]`, `Email Connector`, `Contact Business Relation Link`, `CDO IField Chain Evaluator`). **Prediction: 0 d35 findings disappear on CDO**; reason text can move only via a hidden "+N more" routine. | `cdo/an-{primary,all}.json` |
| P5 goldens/ratchets | No histogram bucket moves (P1b; the design changes completeness only on non-empty, all-real fan-outs and does not touch shared typing). So `CDO_AMBIGUOUS_RESOLVED` (`tests/program_resolve_harness.rs:2898-2905`), the unknown ratchets and the `conditional_resolved` asserts do not move: **no re-pin**. Semantic-edges goldens hold sites and target sets, not completeness (`src/program/resolve/semantic_golden.rs:133-152`). r0-corpus: the only enum fixture (`ws-interface-dispatch`) dispatches through a GLOBAL → open. `tests/fixtures/test-selection-graph/src/Tax.al:26-31` (`TaxRate := Region`, enum parameter, `Extensible = false`) becomes closed; `tests/cli/cli_graph_export.rs:242-250` and `tests/cli/cli_digest_test_coverage.rs:288` stay green (routes unchanged; no open-world there already). `aldump --switch-dump` text gains the new `DispatchMeta` field. | grep/read |

## Non-goals

- **Enums known only from symbols (an app with no source).** The ABI reader
  runs only for apps that ship no source (`src/program/build.rs:259-264`).
  Every measured case has source: the CentralGauge repro, and every closed
  CDO dependency enum in P4b (all in apps whose `.app` carries its `.al`
  files; only `Microsoft_Application` ships none, and it holds none of them).
  A symbol-only enum keeps `closed_enum = false` (the `ObjectNode` default),
  so its dispatch stays open. No `symbol_reference.rs` change. Stated limit;
  follow-up in Discovery D1.
- **Narrowing routes to the enum's mapping.** Routes stay "every loaded
  implementer of I". §2 now loads the mapping anyway (for the proof), so
  narrowing is a few lines, but it changes target sets (graph export,
  semantic goldens) and buys nothing measured (P4a: 2 = 2). Follow-up issue;
  T14 pins today's limit (an unmapped implementer that commits still makes d35
  say Yes).
- Receivers that are parameters, `var` parameters, globals, return values or
  list elements; values from codeunits, other interfaces or
  interface-returning calls (`GetEndpointCreator(...)`): unchanged.
- `--single-app` sites with dependency implementers outside the model: stay
  open (§5).
- The `opaque-callee` uncertainty on interface edges (`db_effect_solver.rs:1071`):
  unchanged. It caps confidence, not coverage; dropping it for closed sites
  would move confidence on every detector and is a separate decision.
- Call-graph export: kind/confidence unchanged (`src/program/impact_graph/project.rs:517`);
  `site_open_bounds` reads routes, not completeness (`full.rs:379-399`).
- No new bucket, no histogram change, no change to the shared receiver typing
  (`receiver.rs:1612-1616` and the `FromInteger` arm stay as they are).

## Acceptance matrix

| # | Issue acceptance item | Pinned by |
|---|---|---|
| A1 | `Extensible = false` repro: candidates present, no open-world uncertainty | T1 (helper), T2 (direct): snapshot `candidates == [Demo Impl.NextKm]`, `openWorld == false`; `closed_over == [Enum 50102]`; no `interface-open-world` in the routine's uncertainties |
| A2 | d35 reports no finding | T1, T2 (default selection) |
| A3 | `Extensible = true` stays open | existing `centralgauge_repro_is_unknown_at_medium` / `..._direct_...` (`tests/cli/cli_d35_unknown_commit.rs:523-540`), unchanged |
| A4 | parameter / codeunit receiver unchanged | T5–T8, T6b |
| A5 | absent `Extensible` = closed, only when provable | T4 (absent), T15 (`#if` arms disagree), T16 (whole-object alternatives), T17 (recovered enum file), T18 (recovered routine file) |
| A6 | CDO ratchets/goldens move only where enum-sourced dispatch exists, each triaged | M1–M4; `scripts/check-goldens` without regen; `scripts/cdo-gate` without re-pin |
| A7 | dependency enums | Revised (coordinator decision, round 2): a dependency enum WITH source closes exactly like a workspace one (T19: dependency-source enum, symbol-only implementer, edge `Complete`); a symbol-only enum stays open (T9: `abi_ingest` leaves `closed_enum = false`). T20: a mapped implementation missing from the graph keeps the site open. ABI closure is Discovery D1 |
| A8 | RHS shapes, with identity | T3 (shapes), T21 (cross-app same-name closed vs extensible) |
| A9 | prove/digest do not regress | T22 |

## Design

### 1. The enum fact (`ObjectNode`)

Add two fields to `ObjectNode` (`src/program/node_extract.rs:127-183`):

- `pub closed_enum: bool` — `true` ONLY for an `Enum` that is provably not
  extensible AND whose implementation map (below) was read completely. Default
  `false` everywhere (all `ObjectNode {` literals add `closed_enum: false`; the
  compiler lists them).
- `pub enum_impls: Vec<(SharedStr, ObjectRef)>` — `(interface folded name,
  implementing codeunit reference)` for every value's mapping, every
  `DefaultImplementation` and every `UnknownValueImplementation`, union over
  `#if` arms. Empty unless `closed_enum` (the dedup below keeps this true).

**Source** (`extract_nodes`, `node_extract.rs:759`):
- not extensible: no `obj.properties` entry `extensible`, or every entry's
  value folds to `false`. Any `true`, any other value → open.
- clean: `!parse_incomplete` (the file parsed `Clean`; P3b).
- map complete: new IR (below). For each interface in `obj.implements` and
  each value: an `Implementation` pair for that interface, else a
  `DefaultImplementation` pair for it. A value with neither, or a pair that
  does not parse as `"<iface>" = "<codeunit>"`, → `closed_enum = false`
  (cannot compile cleanly — AL0596 — so this only fires on odd input).
- New IR: `ObjectDecl.enum_values: Vec<EnumValueDecl>` with
  `EnumValueDecl { name: String, implementations: Vec<String> }` (the raw
  `Implementation` property value per `#if` arm), filled by the lowerer from
  each `enum_value_declaration` body (and inside `#if` wrappers, as
  `collect_properties` does) — `crates/al-syntax/src/ir/decl.rs:9-58`,
  `crates/al-syntax/src/lower/mod.rs:320-335`. Object-level
  `DefaultImplementation`/`UnknownValueImplementation` are already in
  `properties`. No grammar change; review by `lowerer-reviewer`.

**ABI** (deferred, Non-goals): `src/program/abi_ingest.rs:515-540` sets
`closed_enum: false` and `enum_impls: Vec::new()` for every symbol-only
object. No change to `src/engine/deps/symbol_reference.rs`.

**Pack**: `PACK_SCHEMA` 6 → 7 (`src/program/pack/mod.rs:65-71`, doc line
"7: `ObjectNode::closed_enum`/`enum_impls`"); packs hold source-extracted
`ObjectNode`s, so the new fields are payload.

**Dedup** (`src/program/build.rs:307-308` and `:370-371`): replace
`objects.dedup_by(|a, b| a.id == b.id)` with a dedup that, on equal ids,
ANDs `closed_enum` into the retained object and unions `enum_impls`, then
CLEARS `enum_impls` when the AND result is `false` (opus N2), so a closed arm
never survives an extensible one (sol 2) and the "empty unless closed"
invariant holds after the merge.

### 2. Is the implementation set loaded? (sol 5)

A non-empty, all-real fan-out does not prove that every implementation was
loaded: `implementers_of` only indexes codeunits present in the graph, and a
missing one leaves no route to reject (P3b). So the proof checks the enum's
OWN references: for interface I, every `(I, ref)` in `enum_impls` resolves
`Unique`ly FROM THE ENUM'S APP (`index.resolve_object_ref(graph,
enum.id, ObjectKind::Codeunit, ref)`) to a codeunit that is the object of a
real route on this edge. A route's object is `id.object` for
`RouteTarget::Routine(id)`, and for `RouteTarget::AbiSymbol{key}` (a
symbol-only implementer of a source enum) it is `ObjectNodeId { app: key.app,
kind: object_kind_from_abi_type(&key.object_type), key: key.object_node_key() }`,
the conversion `program_calls.rs:2058-2063` already does (opus N1; share one
helper, do not copy it). Without it such a site would silently stay open. If any reference does not resolve, or resolves to an
object no route reaches, the site stays open. The check must not pass
vacuously (opus N3): `enum_impls` must hold at least one entry for EVERY
interface in the enum's `implements` (so in particular for I); a closed enum
with no entry for some implemented interface leaves the site open. §1 already
refuses `closed_enum` for an incomplete map, so this is defence in depth for
any producer that sets the fields directly (T34). Routes may hold extra
implementers (superset, non-goal); they never make it unsound.

### 3. The data flow: `closed_enum_sources`

New `pub(crate) fn closed_enum_sources(...) -> Option<Vec<ObjectNodeId>>` in
`src/program/resolve/receiver.rs` (near `infer_receiver_type_for_expr`,
`:1443`). It returns the enum ids when ALL of these hold, else `None`:

1. **Clean.** The call site's file parsed `Clean` (`file.parse_status`) and
   `!routine.parse_incomplete` (sol 3, opus 4b).
2. **Local.** The receiver expression, after removing parentheses, is an
   `Identifier`/`QuotedIdentifier` V (folded). Every `routine.locals` entry
   named V (all `#if` arms, opus 12) is declared `Interface I` for the same I,
   there is at least one, and no parameter, return name or object global is
   named V.
3. **Allowlist over the routine body** (sol 6, opus 3). Walk every block,
   every `#if` branch, every statement and every expression in
   `routine.body`:
   - the routine contains no `StmtKind::With`, no `StmtKind::Unknown`
     (`crates/al-syntax/src/ir/stmt.rs:76,94`) and no `ExprKind::Unknown`
     (`crates/al-syntax/src/ir/expr.rs:73`);
   - every occurrence of V (identifier or quoted, folded, parentheses
     removed) is EITHER the target of a non-compound `Assignment` OR the
     object of a `Member` that is the function of a `Call` (`V.M(...)`).
     Anything else — an argument (bare or parenthesized, P2e), a `foreach`
     variable, an operand, an `exit(V)`, a compound `+=`, `V.M` without a call
     — leaves the site open;
   - there is at least one assignment to V.
4. **Every assignment's value has a closed enum IDENTITY** (sol 1). The value
   is typed by a small identity typer inside this function — not by
   `EnumType{name_lc}` — that returns the `ObjectNodeId` of the enum, resolved
   in the context that DECLARED the type:
   - a local, parameter, return name or global of the routine/object declared
     `Enum X` → resolve X from the calling object (`from_object.id`), where
     the declaration lives;
   - `Rec.Field` / `Rec."Field"` (a bare `Member`, never a `Call`) where
     `Rec` types as `Record{table: Some(id)}` (an existing identity), AND
     `!index.table_scope_has_routine(graph, from_object, id, field_lc)` — the
     existing field/procedure collision guard (`index.rs:1032`, used by
     `receiver.rs:1963-2009`): a parens-less call to a same-named table
     procedure has the same AST shape (sol S1) → the field's DECLARING object
     (the table or the table extension that declared it; extend
     `field_in_table`, `index.rs:924`, to also return its owner id) → resolve
     the field's `Enum X` from THAT object's id;
   - `Enum::"X"::V`, `"X"::V`, `Enum::"X".FromInteger(n)` → resolve X as an
     Enum from the calling object; `"X"::V` only when no local, parameter or
     global is named X. `FromInteger` is recognised here only, not in the
     shared typing (opus 5);
   - a bare call `F(...)`: keep the existing call-result guards
     (the shadow guard at `receiver.rs:2620-2633`, sol S1). Decline when F
     names any parameter, local, object global OR the routine's named return
     value (the existing guard does not check the return name; this one adds
     it) (an
     in-scope array `F` makes `F(1)` an index, not a call). Then ask
     `resolve_bare` (the same query `infer_call_result_receiver` makes) and
     require exactly one `RouteTarget::Routine(id)` route; read that
     routine's declared return type from its own declaration and resolve
     `Enum X` from `id.object` (the declaring object). A `Builtin`,
     `AbiSymbol`, `Unresolved` or ambiguous result declines;
   - anything else → `None`.
   Each resolution must be `Unique` and land on an object with
   `closed_enum == true` that `implements` I (folded compare). Two different
   closed enums are fine (the result lists both); one non-closed → `None`.

### 4. The edge: `Complete`, routes unchanged

In `full.rs`'s `CalleeShape::Member` arm (`:670-736`): when `recv` is
`Interface{name_lc}` and `closed_enum_sources` is `Some(enums)`, AND the
routes are non-empty, every route is real (no `RouteTarget::Unresolved`, no
`Evidence::Unknown`), AND §2 holds for every enum, set
`completeness = SetCompleteness::Complete` instead of
`completeness_for_shape(member_shape)` (`:736`) and record
`SiteFacts.interface_closed_over = enums` (new field, `:134-160`; empty =
open). Closed + empty would classify `Unknown` (`edge.rs:677`), which the
non-empty guard prevents.

### 5. The model: `closed_over`

`DispatchMeta` (`src/program/model/calls.rs:19-27`) gains
`pub closed_over: Vec<String>` (model ids of the enums; empty = open). In
`program_calls.rs` (`:2101-2120`) fill it from
`site_facts.interface_closed_over` ONLY when `dependency.is_empty()` and the
computed `unresolved_impls` is empty — the same rule
`test_coverage.rs:502-515` uses (sol 7, opus 2). A `--single-app` site whose
implementers are outside the model therefore stays open everywhere
(snapshot, combined graph, test coverage agree). Update
`src/program/model/program_calls_adapter_tests.rs:405-410`.

### 6. The detector layer

- `combined_graph.rs` (`assemble`, `:304-404`): before the edge loop, build
  `closed_sites: HashSet<&str>` from the edges that carry `dispatch_meta`
  with non-empty `closed_over`, using the owning-row rule (`owns`, `:318-323`).
  In the targeted-edge branch (`:358-381`) skip the uncertainty for a closed
  site; do not insert it into `interface_open_world_emitted`. The to-less
  branch is unreachable for a closed site (§5) and stays as is.
- `snapshot.rs:2201`: `open_world: Some(!closed)` from
  `representative.dispatch_meta` (the meta-carrying edge, `:2184`).
- `unresolved_cone.rs:147-149`: also skip a row with
  `status == "polymorphic"`, `open_world == Some(false)` and no
  `unresolved_candidates` (opus 1), so `prove` (`prove.rs:233-251`) and
  `digest_cli.rs:1148` see no blocker for it — a closed site is a known
  candidate set, not an unresolved call.
- `test_coverage.rs:502-515`: unchanged (already agrees by construction).

Then d35 needs no change: with nothing else open, the routine's coverage
stays `complete`, the inherited cone is complete, and `may_commit_derived`
answers `No` — or `Yes` when a candidate commits.

### Bucket

`SetCompleteness::Complete` ("provably exhaustive", `edge.rs:139-146`) on the
unchanged `Polymorphic` edge, plus `closed_over` in the model. Not
`conditionalResolved` (manual-binding routes, `edge.rs:651-655`) and not
`ambiguousResolved` (same-object overloads). Both reviewers agreed. Model
`Resolution::Maybe` per candidate stays: closedness belongs to the site.

## Panel round 1: what changed, point by point

| Point | Verdict | Change |
|---|---|---|
| sol 1 (identity) | accepted; verified `receiver.rs:1994-2009` + `graph.rs:90-138` | §3 rule 4 identity typer in the declaring context; T21 |
| sol 2 (`#if` objects) | accepted; verified `build.rs:307-308,370-371` | §1 dedup ANDs; T16 |
| sol 3 / opus 4b (recovery) | accepted; `snapshot/parse.rs:39-58` | §1 clean enum, §3 rule 1 clean routine; T17, T18 |
| sol 4 / opus 4a (ABI states) | accepted; `raw_object_property` first-match/None confirmed | `closed_enum: bool` default false; the four-state reader moved to Discovery D1 in revision 3 (ABI deferred) |
| sol 5 (loaded set) | accepted | §2 checks the enum's own references; new `enum_impls` + lowered enum values; T20 |
| sol 6 / opus 3 (escapes) | accepted as allowlist; the by-reference `(V)` case is refuted by alc AL0130 (P2e) but declined anyway | §3 rule 3; T8, T8b, T23–T26 |
| sol 7 / opus 2 (to-less) | accepted | §5 rule; M2 restated (single-app 0, cross-app 2 predicted); T27 |
| sol 8 / opus 9, 10 (tests, per-site) | accepted | §6 per-site set from the meta edge; T10–T14, T19, T6b, T28–T32 |
| sol 9 / opus 8 (pack) | accepted | `PACK_SCHEMA` 7; T31 |
| opus 1 (prove/digest) | accepted; verified | §6 `unresolved_cone`; T22 |
| opus 5 (FromInteger) | accepted | recognised only in §3 rule 4; shared typing untouched |
| opus 6 (T3 break) | accepted; moot | no shared-typing change any more; T3's break is now "drop the `Enum::"X"::V` arm from the identity typer" (one case fails) |
| opus 7 (paths) | accepted | full paths throughout |
| opus 11 (M3 scope) | accepted | M3 triages every detector |
| opus 12 (locals in arms) | accepted | §3 rule 2 |

## Panel round 2 and the scope decision: what changed

| Point | Verdict | Change |
|---|---|---|
| Scope (both reviewers, coordinator decision) | ABI `Values` ingestion deferred | Non-goal + Discovery D1; removed the `RawObject.Values`, positional-map and `Extensible` reader edits, the ABI halves of T9/T19/T20/T32; A7 and F3 revised |
| sol S1 (binding guards) | accepted; verified `receiver.rs:1963-2009`, `index.rs:1032`, `receiver.rs:2620-2633` (citations corrected in revision 4) | §3 rule 4 keeps the field/procedure collision guard and the call-shadow guard (params, locals, globals, named return) and asks `resolve_bare` for the callee; T9b, T9c, T9d |
| sol S8 (T6, T8b) | accepted | T6 is a baseline control; T6b carries the discrimination; T8b's break ignores parenthesized argument occurrences and must yield a wrongly closed site |
| opus N1 (`AbiSymbol` routes) | accepted; verified `program_calls.rs:2058-2063` | §2 converts `AbiSymbol{key}` with the shared helper; T19 pins it |
| opus N2 (dedup invariant) | accepted | §1 dedup clears `enum_impls` when the merged object is open; T33 |

## False-positive and failure shapes

| Shape | Result | Guard |
|---|---|---|
| Field of a dependency table typed with the dependency's extensible `K`, caller app has its own closed `K` | open | §3 rule 4 (declaring context) |
| Whole-enum `#if` alternatives, one extensible | open | §1 dedup AND |
| Recovered enum file / recovered routine file | open | §1 clean, §3 rule 1 |
| Any symbol-only enum (no source), whatever its `Extensible` | open | Non-goals (deferred, D1) |
| `Rec.Kind` is really a parens-less call to a table procedure `Kind` | open | §3 rule 4 collision guard |
| `F(...)` is really an index into a same-named array / named return | open | §3 rule 4 shadow guard |
| A mapped implementation missing from the graph | open | §2 |
| V reassigned from a codeunit / other interface / interface-returning call | open | §3 rule 4 |
| V passed as an argument (bare, parenthesized, quoted), `foreach` variable, operand, `exit(V)` | open | §3 rule 3 |
| Any `with`, any `Unknown` statement/expression in the routine | open | §3 rule 3 |
| V a parameter (even if reassigned in the body), global, return name; a same-named non-interface local in another `#if` arm | open | §3 rule 2 |
| Two closed enums | closed, both listed | §3 rule 4 |
| Closed + extensible enum | open | §3 rule 4 |
| Unknown ordinal (`FromInteger(7)`, assignment-compatible conversion) | `UnknownValueImplementation` (in `enum_impls`, so it must be a loaded route) or a runtime error | §2 |
| Implementer lacks the method | open | §4 |
| Dependency implementers outside the model (`--single-app`) | open | §5 |
| An unmapped extra implementer that commits | d35 `Yes` (stated limit, T14) | non-goal |

## Tests (each with a discrimination break)

d35-level tests extend `tests/cli/cli_d35_unknown_commit.rs` (inline tempdir
workspaces, `:360-540`; no r0-corpus fixture → no golden family).
Resolver-level tests go in `src/program/resolve/full.rs` / `receiver.rs`
tests with the existing in-memory multi-app builders (`index.rs` `build_fixture`
style). Every break is made with Edit, run red, reverted with Edit, run green
(the `discrimination-proof` skill); both outcomes are recorded in the PR.

| Test | Assert | Break that must turn it red |
|---|---|---|
| T1 CG repro closed, call in `IsDue` | no d35; `openWorld false`; one candidate | delete the `combined_graph` skip |
| T2 CG repro closed, direct | no d35; edge `Complete` | restore `completeness_for_shape` while leaving `closed_over` filled → the `Complete` assert fails (completeness pinned on its own) |
| T3 RHS shapes: enum local, enum parameter, record field, `"K"::V`, `Enum::"K"::V`, `Enum::"K".FromInteger(n)`, own-object call returning the enum | each closed | drop the `Enum::"K"::V` arm of the identity typer → that case fails |
| T4 `Extensible` absent | closed | treat absent as extensible |
| T5 `S := Rec.Kind; S := SomeCodeunit;` | open | check only the first assignment |
| T6 interface parameter, no body assignment | open | baseline control, no break claimed: it also fails the "at least one assignment" rule, so accepting parameters keeps it green (sol S8) |
| T6b interface parameter ALSO assigned `S := Rec.Kind` in the body | open | accept parameters in rule 2 → wrongly closed (this test carries the parameter discrimination) |
| T7 global assigned from a closed enum in this routine | open | accept globals |
| T8 `Foo(S)` after a closed assignment | open | allow argument occurrences |
| T8b `Foo((S))` (value parameter) after a closed assignment | open | make the occurrence walk ignore an occurrence inside a parenthesized call argument (do not visit it) → the site comes out wrongly CLOSED (sol S8; merely skipping parenthesis removal still rejects it, so that break proves nothing) |
| T9 symbol-only enum (no source, `Extensible "0"` in its symbols) in a dependency, workspace dispatch from a field of that type | open; `closed_enum == false` on its `ObjectNode` | n/a, baseline control for the deferred ABI path (break: set `closed_enum: true` in `abi_ingest` → wrongly closed) |
| T9b field/procedure collision: `S := Rec.Kind` where the field `Kind` is of a CLOSED enum and the table also declares a procedure `Kind()` returning an EXTENSIBLE enum | open (the guard declines the field arm) | drop the `table_scope_has_routine` guard → the field arm reads the closed field → wrongly closed (types swapped in revision 4, sol round 3: the earlier orientation stayed open without the guard and proved nothing) |
| T9c shadowed call: `S := GetKind(1)` where `GetKind` is also a local `array[2] of Enum "Open Kind"` and an own-object procedure `GetKind(i)` returns a closed enum | open | drop the shadow guard (or only its named-return part, T9d) → wrongly closed |
| T9d named-return shadow: the routine's named return value is `GetKind` | open | drop the return-name part of the shadow guard → wrongly closed |
| T10 Tax fixture `cli_graph_export.rs:242-250` | unchanged OverApprox edges | narrow routes |
| T11 closed, one implementer lacks the method | open | drop §4's all-real guard |
| T12 TWO candidates (two values → two codeunits), detector level | no `interface-open-world` from either edge | skip only on the meta-carrying edge |
| T13 closed, one candidate commits | d35 `Yes` (high, via callee) | suppress the candidate edge → finding disappears |
| T14 extra implementer of I not mapped by the enum commits | d35 `Yes` (pins the non-goal limit) | narrow routes → finding disappears |
| T15 `#if` arms disagree on `Extensible` | open | take the first property entry |
| T16 two whole-enum `#if` alternatives, one `Extensible = true` | open | restore plain `dedup_by` |
| T17 enum file recovered (syntax error elsewhere in it) | open | drop the `parse_incomplete` check |
| T18 caller file recovered | open | drop rule 1 |
| T19 source enum (`Extensible = false`) whose only implementer is a symbol-only dependency codeunit (route `AbiSymbol{key}`), resolver level | edge `Complete`, `interface_closed_over` set (the model still withholds `closed_over` by §5 when that implementer is outside the model) | match only `RouteTarget::Routine` in §2's route-object helper → edge stays `Partial` (opus N1) |
| T20 source enum maps two codeunits, one not loaded (its file excluded from the build), one loaded | open | skip §2 → wrongly closed over the one loaded route |
| T21 cross-app same name: dependency table field typed with the dependency's extensible `K`; workspace declares a closed `K` | open | resolve the field type from the caller |
| T22 prove/digest on the closed repro | no `unresolved-callsite` blocker; digest row has no blocker | drop the `unresolved_cone` skip |
| T23 `foreach S in List` | open | allow foreach occurrences |
| T24 `with R do ...` anywhere in the routine | open | drop the `With` rule |
| T25 quoted target `"S" := SomeCu;` after `S := Rec.Kind` | open | match unquoted identifiers only |
| T26 routine with an `Unknown` statement (hand-built IR) | open | drop the `Unknown` rule |
| T27 a dependency implementer outside the model (`--single-app`) plus a workspace one | open, `openWorld true` | drop §5's `dependency.is_empty()` |
| T28 conditional writes `if c then S := A.Kind else S := B.Kind` (same closed enum) | closed | require a single assignment → red |
| T29 two closed enums assigned on different paths | closed, `closed_over` lists both | keep only the first enum |
| T30 closed + extensible enum writes | open | accept when any write is closed |
| T31 pack round trip with `closed_enum: true` and `enum_impls` | equal after decode; `PACK_SCHEMA == 7` | drop the field from serde |
| T32 `DefaultImplementation` value and `UnknownValueImplementation` (source enum) | closed; both codeunits in `enum_impls` and required as routes; with the Default codeunit not loaded → open | omit Default from `enum_impls` → the not-loaded variant comes out wrongly closed |
| T34 hand-built `ObjectNode` for an enum implementing I and J with `closed_enum: true` but `enum_impls` holding only J entries; dispatch over I from that enum | open | drop §2's "at least one entry per implemented interface" check → passes vacuously → wrongly closed (opus N3) |
| T33 dedup of two whole-enum `#if` alternatives, one extensible | merged object has `closed_enum == false` AND `enum_impls` empty | drop the clear step (opus N2) → `enum_impls` non-empty |

## Measurement plan

Release-fast build of the branch vs master 9bd2996f, on
`/work/src/cdo-baseline/Cloud` (`bc3ccb18`, clean).

- **M1** `aldump --program-call-graph-stats`: whole and primary histograms
  byte-identical (whole `total 46328, unknown 0, ambiguousResolved 23,
  conditionalResolved 266, resolvedSource 11568`; primary `total 20916`). Any
  move → F1.
- **M2** `aldump --switch-dump` + `--switch-compare` (single-app model): the
  only `calls.edges` difference is the new field; **0** sites with non-empty
  `closed_over` (P4a). Cross-app: count closed sites in the `analyze` model
  (a temporary census counter or the snapshot rows with `openWorld false`);
  the 2 `IPrePostValidator` sites MAY close (CDN ships source, its enum is
  `Extensible "0"`, and the callee is an own-object procedure) but this is
  not promised; plus N dependency sites from source-backed enums. Report the
  number measured, list every site with its enum, and spot-check ≥10 (or all)
  against source.
- **M3** `alsem analyze --format json --deterministic` at `--scope primary`
  and `--scope all` (default cross-app), before/after via the `cdo-measure`
  skill: for EVERY detector, findings added/removed, fingerprints, and moved
  `confidence` / `cappedBy` (`dynamic-dispatch`, `confidence.rs:112`), d1
  cohort reasons (`src/engine/l5/d1_cohort.rs:794-802`) and coverage reasons.
  Each moved item is triaged against source. Predicted: d35 52/162 unchanged.
- **M4** `scripts/check-goldens` without regen: green. `scripts/cdo-gate`:
  green, no pin edit.
- Repro: `/work/bench/256/repro-{closed,direct-closed}` → no d35;
  `repro-{ext,direct}` → unchanged medium finding.

## Pre-stated fallbacks (stop and revise, never a silent reduction)

- **F1** A histogram bucket or a CDO ratchet moves: stop; find what reads
  completeness or typing beyond P1b, revise this spec. Never re-pin to green.
- **F2** M3 shows a finding disappear or a confidence move on any detector:
  triage it against source before accepting; record it here.
- **F3** (revised) The identity typer misses a common P4b shape
  (dependency-table field, `FromInteger`, enum global), or M2 shows that a
  closed site needs a symbol-only enum: extend the typer with its own test
  and binding guards (§3 rule 4), and for a symbol-only enum stop and take up
  Discovery D1 as its own spec. Never accept a name-only or untyped value, and
  never close a symbol-only enum without its implementation map.
- **F4** A golden moves under `check-goldens`: stop and explain before any
  regen; P5 predicts none.
- **F5** Lowering enum values touches a golden (`tests/ir-l2-goldens/`) or the
  lowerer reviewer finds a shape hazard: stop, revise §1 (a source enum
  without a lowered map stays open).
- **F6** An unmapped implementer changes a CDO finding (T14's limit seen in
  M3): route narrowing becomes in scope; revise this spec first.

## Discovery

D1, the ABI follow-up (file as its own issue):

```
subsystem: program::abi_ingest / engine::deps::symbol_reference (dependency enum facts)
locator: src/engine/deps/symbol_reference.rs:494-513 (RawObject has no Values), :1453-1467 (EnumTypes reader), :520-532 (raw_object_property first match, None for a valueless entry); src/program/abi_ingest.rs:515-540
symptom: an interface call whose value comes from a non-extensible enum declared in a dependency that ships no source (symbols only) stays open-world (interface-open-world; d35 unknown), because #264 closes only source enums
kind: capability gap (precision)
origin_issue: 264
reproducer: the CentralGauge #256 repro split into three apps, so that ONLY the enum is symbol-only. App S ships SOURCE: interface "Demo Strategy" and codeunit "Demo Impl". App E depends on S and ships its .app WITHOUT source: enum "Demo Kind" (Extensible = false, value Standard -> "Demo Impl"). E must depend on S because an enum can only name codeunits its app can see. The workspace depends on E and S and keeps the table, "Demo Publisher" (IsDue) and "Demo Subscriber". In the default cross-app analyze, "Demo Impl"'s body comes from S's source, so the edge has a target and the cone can be complete. Today alsem analyze still reports the d35 unknown finding with reason interface-open-world. (Chosen over the variant where the implementation is symbol-only too: that body is unavailable, the edge is to-less, §5 withholds closedness, and d35 cannot prove the cone commit-free, so "no finding" would be the wrong acceptance.)
pre_existing: yes (master 9bd2996f; #264 leaves it unchanged by decision)
capability: a symbol-only enum is closed when its symbol Extensible property is absent or "0"/"false" (every entry, no conflicts, no valueless entry) and every value's Implementation (comma list positional over ImplementedInterfaces), DefaultImplementation and UnknownValueImplementation resolve to loaded codeunits; the same §2 loaded-set check then applies
acceptance: on the three-app reproducer the NextKm edge is Complete with closed_over [Enum "Demo Kind"] and d35 gives no finding; "1", valueless, unknown text and conflicting duplicates stay open; a mapped implementation absent from the symbols stays open; a count mismatch between Implementation and ImplementedInterfaces stays open; PACK_SCHEMA is unchanged if the ObjectNode fields from #264 suffice
```

## Files the implementer touches

- `crates/al-syntax/src/ir/decl.rs:9-58` (`ObjectDecl.enum_values`, `EnumValueDecl`);
  `crates/al-syntax/src/lower/mod.rs:320-335` (collect enum values).
- `src/program/node_extract.rs:127-183` (fields), `:759` (source fact); every
  `ObjectNode {` literal (compiler-guided).
- `src/program/build.rs:307-308`, `:370-371` (AND-dedup).
- `src/program/abi_ingest.rs:515-540` (`closed_enum: false`, empty `enum_impls`).
- `src/program/model/program_calls.rs:2058-2063` (lift the `AbiSymbol` →
  `ObjectNodeId` conversion into a shared helper for §2).
- `src/program/pack/mod.rs:65-71` (`PACK_SCHEMA` 7).
- `src/program/resolve/index.rs:924` (`field_in_table` returns the owner too, or a sibling fn).
- `src/program/resolve/receiver.rs` (new `closed_enum_sources` + identity typer near `:1443`).
- `src/program/resolve/full.rs:134-160` (`SiteFacts`), `:670-736`.
- `src/program/model/calls.rs:19-27`; `src/program/model/program_calls.rs:2101-2150`;
  `src/program/model/program_calls_adapter_tests.rs:405-410`.
- `src/engine/l4/combined_graph.rs:304-404`.
- `src/engine/l5/snapshot.rs:2184-2201`.
- `src/engine/l5/unresolved_cone.rs:147-149`.
- Tests: `tests/cli/cli_d35_unknown_commit.rs`; resolver/ingest/pack unit tests;
  a prove/digest test beside the `src/engine/l5/prove.rs` tests.
- `CHANGELOG.md` (Fixed).
