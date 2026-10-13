# Call-graph export for diff-scoped test selection: design (rev 2, 2026-10-13)

Rev 2 applies the spec review's findings: pruning keeps hidden-callers nodes and
states the universal-hub rule (4.6), name hubs for record-operation and run member
names (4.5), the separate test-app limit (6, 8, 9), the cross-app composition ends
before the demand filter (7), publishers are excluded from the opaque-body rule
(4.5), the callbacks hub includes the app itself (4.5), the unexercised
`AmbiguousOverload` row (4.3), integer run targets are dynamic (fact 3), the
`Call`-to-trigger pin (15), and the join's path and quoted-name comparison (8).

Issue #63. Consumer: `bcdev_test_select` in bc-dev-mcp (epic SShadowS/bc-dev-mcp#17,
child c03.4). We own both repos, so this spec designs both sides. It builds on the
2026-10-13 investigation of #63 (validity check, research answers Q1-Q7) and checks
every claim it relies on against the code at master `3ec8cc68` (al-sem) and `6873a41`
(bc-dev-mcp).

## 1. Goal

`alsem graph <workspace>` writes the program's call graph as JSONL. bc-dev-mcp walks
it backwards from changed procedures to the `[Test]` methods that can reach them.

**Done means** (the #63 acceptance, verified in bc-dev-mcp):

1. On `tests/fixtures/test-selection-acceptance`, with **no** coverage map,
   `Acc Nightly Recalc.ApplyDecay` (reached only through `Codeunit.Run`) selects
   `Acc Loyalty Tests.NightlyRecalcDecays`. The `OnRun` trigger change (D5) selects
   the same test instead of falling back to the whole suite.
2. Event and interface edges are present and labelled `Event` and `Interface`.
3. Dependency routines are in the graph when the dependency ships source. What happens
   for symbol-only dependencies is defined (section 6) and tested.
4. The consumer stays **sound**: wherever the graph cannot show every way a routine
   can run, the export says so, and the consumer falls back or leans on coverage
   history. It never reports such a routine as "uncoverable".

Terms used below:

- **program**: the workspace app plus the dependency apps in its declared closure.
  Apps that depend on the workspace are not part of it (engine-switch S7.4).
- **sound** (for selection): every test that can execute a changed routine is
  selected, or the result says it could not decide (`fallbackToAll`).
- **open site**: a call site whose possible targets the engine cannot list (unknown,
  dynamic, or a call into code that has no source).
- **hub**: an extra node in the export. It stands for the set of routines an open
  site might reach, so the consumer's ordinary backwards walk handles open sites.
- **hidden callers**: a way a routine can be invoked that the graph does not show as
  an edge (for example, a platform event that no edge raises yet).

## 2. What the consumer does today

- **Provider.** `createAltoolCallGraphProvider` (`src/core/call-graph.ts:168`) runs
  `altool graph extract` + `stitch`. `parseCallGraphJsonl` (`:66-103`) reads lines
  `{"t":"node"|"edge","v":{...}}`, ignores other `t` values (`:100`) and throws on an
  edge kind outside `Direct|Event|Interface|Run|Trigger` (`:97`). The file header
  (`:13`) says "al-sem has no usable CLI, so altool is the only engine".
- **Join.** `indexGraph` (`src/core/test-selection.ts:86-99`) parses altool's node id
  with `NODE_IDENTITY` (`... # Name#methodId`, `:76`) and joins on
  `(objectType, objectId, methodId)`, else on the name (every overload).
- **Discovery.** `discoverAlProcedureIdentities` (`src/core/al-procedures.ts:1301`) is
  a token scanner over workspace files only (`.alpackages` is skipped, `:157`,
  `:1101`). A procedure gets `startLine` (its first attribute line, else its access
  modifier, `declarationStart`, `:660-680`), `endLine`, and a computed BC `methodId`
  (`calculateProcedureMethodId`, `:1058-1099`; `null` when any input is unresolved).
  Triggers are reported as `unsupportedExecutables` (`:826-871`), and every changed
  trigger forces `fallbackToAll` (`test-selection.ts:114-116`). `TestMethodIdentity`
  (`:1422`) has no location.
- **Selection** = static reverse reachability ∪ coverage history (keyed by BC
  methodId). A changed procedure that nothing reaches is reported `uncoverable`; that
  is not a fallback (`test-selection.ts:191`). With altool it means "no evidence", not
  "proven": altool says nothing about open sites.
- **The graph is skipped** when no procedure changed (`src/mcp/tools/test-tools.ts:262-263`).

Measured altool gaps on the acceptance fixture (fixture README): no `Run` edge
(`Codeunit.Run` is a `Direct` edge to the built-in `Codeunit # Run`), no symbol-only
nodes, and trigger changes fall back. Without a coverage map, `ApplyDecay` is reported
`uncoverable` although `NightlyRecalcDecays` reaches it: a silent miss.

## 3. Facts this design rests on (checked 2026-10-13)

The investigation's open questions are answered here (Q1-Q7), plus what this design
pass found.

1. **The engine already has the edges.** On a copy of the acceptance fixture,
   `aldump --graphify-export` shows `RecalcNightly -> OnRun` (`runs`, resolved,
   exact), `TaxAmount -> RatePct` x3 (polymorphic) and 2 event flows. Walking it
   backwards reproduces D1-D6 with no coverage map (investigation).
2. **Q1, event flow.** `emit_event_flow_edges` runs over every app's routines
   (`src/program/resolve/full.rs:1197-1209`). `ProgramContext::resolve_dependency_bodies`
   (`full.rs:1544`) adds only call-site edges (Call, Run, ImplicitTrigger). There is no
   duplication. `resolve_full_program_from_parts` is private; `resolve_full_program_with`
   (`:1291`) is the public entry. `alsem analyze`'s cross-app build composes report +
   dependency bodies in `assemble_and_resolve_cross_app_program`
   (`src/program/model/program_calls.rs:799-835`).
3. **Q4, runs.** `EdgeKind::Run` comes from `CalleeShape::ObjectRun`
   (`extract.rs:382-411`, `full.rs:645-692`): keyword form `Codeunit/Page/Report/XmlPort.Run`,
   `Page/Report.RunModal`, `XmlPort.Import/Export`; the record argument is ignored. A
   typed variable (`Cu.Run()`, `PageVar.RunModal()`) arrives as `EdgeKind::Call`
   routed to the entry trigger by `dispatch_entry_trigger` (`resolver.rs:2034-2113`).
   Either way the target is the **entry trigger only** (`OnRun`, `OnOpenPage`,
   `OnPreReport`, `OnPreXmlPort`, `resolver.rs:1994-2002`), plus the entry triggers of
   page/report extensions. An object with no entry trigger gives an empty edge, and
   `SiteFacts::run_target` names the object (`full.rs:132-136`). Only an
   `ExprKind::DatabaseReference` argument (`Codeunit::X`, `Codeunit::"X"`) is a static
   target (`extract.rs:383-386`); a bare integer (`Codeunit.Run(50000)`) is not, so it
   resolves `DynamicOpen` (`resolver.rs:2203-2210`) and becomes a `Run{kind}` hub to
   every codeunit in the program (4.5): sound, but coarse. (The resolver doc at
   `resolver.rs:2123` that calls `Codeunit.Run(50000)` static is wrong and is corrected
   with this work.)
4. **Q2, lines.** `RoutineMeta.origin` (decl surface) exists for workspace and
   dependency routines. Its row is the `procedure`/`trigger` keyword or the access
   modifier; attributes are not included (`AttributeIr` has no origin). altool uses
   the same row (`CashRoundOnInvoiceTotal`: attribute on line 3, node `line` 4). The
   consumer's `startLine` is the first attribute line, so the join must test
   **containment**, not equality. `virtual_path` is relative to the app's source root;
   for the workspace that is the workspace folder, forward slashes
   (`src/snapshot/provider.rs:41`). Workspace and dependency paths can collide.
5. **Q3, member triggers.** The engine keeps the trigger name (`OnValidate`) and the
   field/action/dataitem in `enclosing_member`. The consumer reports only the trigger
   name. Location tells same-named triggers apart.
6. **Q7, symbol-only targets.** An `AbiSymbol` route key maps back to its routine node
   by `(object, name, params_count, sig_fp)`, because an ABI routine's `sig_fp` is
   `param_type_fp` (`abi_ingest.rs:443`, `resolver.rs:183`). A run of a symbol-only
   object targets an `AbiSymbol` entry trigger with no routine node (`resolver.rs:2154`).
7. **BC methodId.**
   - `SymbolReference.json` lists each public/internal method's compiler `Id`, and it
     **is** the BC methodId. The consumer's algorithm reproduces all three `GetNextNo`
     overloads in Microsoft Business Foundation 28.1.49838.50268 (`-1587107043`,
     `1304721844`, `1026401181`, the last with a `var Record "No. Series Line"`
     parameter). Local procedures and triggers are not listed (0 of 173 methods there
     are `IsLocal`). al-sem does not read the field today (`RawMethod`,
     `src/engine/deps/symbol_reference.rs:429`).
   - **Trigger methodIds do not follow the procedure rule.** Under it `OnRun` hashes to
     `289754967`; BC and altool report `-401248892`. No simple variant (casing,
     prefixes, object id, return kinds) matched. Neither side can compute trigger
     methodIds today.
   - The consumer computes methodIds only for workspace procedures. It never needs a
     dependency methodId: diffs and tests are in the workspace.
8. **Open sites that look closed today.**
   - A record operation whose table is not resolved is an empty, open `Multicast`
     (`full.rs:750-760`), which `classify_obligation` calls `HonestEmpty`
     (`edge.rs:625-627`). On the edge it looks the same as a resolved table that has
     no triggers.
   - Catalog builtins that run objects or fire triggers resolve as `Catalog`, not as a
     hole: `PageInstance::run/runmodal`, `ReportInstance::*` (`run`, `runmodal`,
     `saveas*`, `execute`, `print`, ...), `RecordRef` insert/modify/delete/rename,
     `FieldRef` validate, `Session.StartSession`, `TaskScheduler.CreateTask`. Only the
     first four are even audited (`ENTRY_DISPATCH_BUILTIN_IDS`,
     `member_catalog.rs:667-672`).
9. **Channels the engine does not model** (section 9 files them):
   - Record operations never reach the synthetic platform table events
     (`OnAfterInsertEvent`, ...). `inject_platform_event_publishers`
     (`src/program/build.rs:534`) creates those publishers so subscribers bind, but no
     edge calls them (no other use of `PublisherKind::Platform` in the resolver).
   - `resolve_implicit_trigger` (`resolver.rs:2277-2347`) looks up only `On<Op>` in the
     table and its extensions. Table-extension `OnBefore<Op>`/`OnAfter<Op>` and field
     `OnBeforeValidate`/`OnAfterValidate` are never reached
     (`tests/r0-corpus/ws-d40/src/Trigger.al` has a table-extension `OnAfterInsert`).
   - An `[EventSubscriber]` that binds to no publisher has no incoming edge
     (`FreshCoverage::unbound_subscriptions`, `full.rs:1847-1851`).
   - Platform entry points (session and company initialization, global triggers) have
     no callers.
   - A run reaches only the entry trigger, not the rest of the run object's triggers.
10. **Q6, size.** DO, release-fast `aldump --graphify-export` (workspace edges + event
    flow, no dependency bodies): 5.7 s, 127,277 routine nodes (about 121k in
    dependencies), 126 MB of graphify JSON. Dependency-body edges were not measured.
11. **Q5, fixtures.** No test walks `tests/fixtures/` as a whole, so a new fixture
    there is safe. Avoid `tests/r0-corpus/` (it moves three golden families).
12. **Existing pieces to reuse:** `is_framework_invocation_attribute` (the test and
    handler attributes, `src/analysis.rs:343-360`), `sig_fp::write_len_prefixed`
    (injective field encoding), `blake3` (already a dependency), `graphify_export`'s
    `object_kind_str` (`graphify_export.rs:796`), `TrustTier::as_str`,
    `driver_version()`.

## 4. Decisions

### 4.1 Identity and the join

**Node `id`** is al-sem's own opaque, stable string: a 96-bit blake3 hash (24 hex
characters) over the length-prefixed identity tuple, with a type prefix. Routines:
`r:` over `(app guid, object kind, object key, enclosing member, name, params_count,
sig_fp)`. Run objects: `o:` over `(app guid, object kind, object key)`. Hubs: a
readable `h:` id (section 4.5). Ids are unique within a file and stable for the same
input and engine version. They are not a persistence key. Hashing avoids escaping
problems (`"G/L Account"`-style names contain `/`); every node row carries readable
fields, so nothing needs to parse an id.

**The join key for changed code is location**, not methodId. The consumer matches a
discovered procedure or trigger to the workspace node with the same `sourcePath`
whose `line` lies inside the consumer's `[startLine, endLine]`, then checks the name.
This is exact for overloads and for same-named member triggers (fact 5), and it works
for triggers, which have no computable methodId (fact 7).

**`methodId` is emitted too, nullable** (Stage C, section 7):
- symbol-only routines: verbatim from `SymbolReference.json` (the compiler's value);
- workspace procedures: computed by a Rust port of the consumer's algorithm, enabled
  only when it reproduces every comparable ABI method `Id` in the run's dependency
  closure (calibration, section 7);
- triggers: `null` until a validated rule exists (follow-up F6).

**Answer to the design question.** The consumer's own methodId computation is sound
by construction (it returns `null` rather than guess, and a miss falls back to a name
join over every overload), but limited: it exists only for workspace procedures. It
never runs for dependency routines (never needed) and is not defined for triggers, and
the trigger rule is not the procedure rule (fact 7). Emitting methodId from al-sem
does **not** make the graph join exact; location does, triggers included. What it
removes is a different class of mismatch: coverage-history lookups. History is keyed
by BC's runtime methodId, and today a wrong or `null` consumer methodId makes that
lookup miss silently, which weakens the half of the union that backstops dynamic
paths. With al-sem's methodId the consumer cross-checks its own value (a disagreement
is a loud warning), fills its `null`s, and looks history up under every known id. al-sem
is the better authority here: it resolves parameter types with the full program
(namespaces, dependency objects), and it can calibrate itself against hundreds of
thousands of compiler-written ids in the dependencies' symbol files on every run. The
consumer only ever adds evidence from it, so a wrong al-sem value can never drop a
test.

### 4.2 Wire format and versioning

JSONL in the envelope the consumer already reads: one object per line, `{"t":...,"v":{...}}`.
Line types: `meta` (exactly one, first), `node`, `edge`. Field names follow altool's
where they overlap (`id`, `kind`, `objectName`, `objectId`, `memberName`, `sourcePath`,
`line`; `caller`, `callee`, `kind`, `confidence`).

`meta.schema = "al-sem.callgraph"`, `meta.version = 1` (a major version, integer).

- **Additive, no version bump:** new optional fields, new `via` values, new
  `hiddenCallers` values, new hub classes, new `assumptions` entries. The consumer
  must ignore unknown fields and must treat **unknown values fail-safe**: an unknown
  edge `kind` as indirect (never `Direct`), an unknown `confidence` as `Open`, an
  unknown `hiddenCallers` value as hidden callers, an unknown node `kind` as a node
  that only carries the walk.
- **Breaking, bump `version`:** a changed meaning or direction of an existing field or
  edge, or a removed field. The consumer rejects a major version it does not know.
- An old consumer (today's parser) reading v1 output degrades safely: no node id
  matches `NODE_IDENTITY`, so every changed procedure is "not in the call graph" and
  selection falls back to the whole suite.

One format only (no `--format json`); `jq` covers ad-hoc reading.

### 4.3 Edge kinds and confidence

`kind` keeps the consumer's five values; the consumer uses it to tell a direct path
from an indirect one. `confidence` says how sure the edge is:

- `Resolved`: the callee runs whenever the caller executes the site (exact dispatch,
  or a multicast where every route fires, with no conditions).
- `OverApprox`: the callee may run (one of a closed candidate set, a conditional
  route, or a lifecycle/test-framework edge from section 4.4).
- `Open`: the edge stands in for targets the engine cannot list (section 4.5).

Mapping from the engine, by `EdgeKind` first and dispatch shape second (an implicit
trigger is a `Multicast`, so shape alone would mislabel it):

| Engine | Export | Confidence |
|---|---|---|
| `EdgeKind::Run`, or a `Call` route whose target is a trigger (typed-variable run; no other `Call` reaches a trigger) | `Run`, caller -> the run object's node (4.4) | `Resolved` (static target) |
| `Call`, `Exact` | `Direct` | `Resolved` |
| `Call`, `AmbiguousOverload` (each candidate; the review read `edge.rs:103-118`, "NOTHING constructs this shape yet", but that doc is stale: `resolve_in_object`'s `_` arm builds it, and DO has 23 workspace and 459 dependency `ambiguousResolved` edges. The unit test still hand-states the edge, so it does not depend on the resolver producing one) | `Direct` | `OverApprox` |
| `Call`, `Polymorphic` (interface dispatch) | `Interface` | `OverApprox` |
| `ImplicitTrigger` | `Trigger` | `Resolved`, or `OverApprox` with conditions |
| `EventFlow` (publisher -> subscriber) | `Event` | `Resolved`, or `OverApprox` (`manualBinding`) |
| a route with `Evidence::Unknown` / `Unresolved` | never an edge; becomes a hub edge (4.5) | `Open` |
| `Builtin` routes | dropped, except the dispatching builtins of 4.5 | |

All routes are used (`Edge::may_reachable_routes`): a `ManualBinding` subscriber may
be bound by a test, and one `AmbiguousDispatch` candidate always runs. Conditions are
listed on the edge (`conditions: ["manualBinding"]`, ...). Raising an event is a
`Direct` edge from the raising routine to the publisher; the `Event` edge then runs
publisher -> subscriber. No self-edges are emitted. Duplicate `(caller, callee, kind)`
rows merge, keeping the strongest confidence (`Resolved` > `OverApprox` > `Open`) and
the smallest `line` (`Codeunit.Run` with and without a record gives identical rows).

### 4.4 What "can reach" means: the may-execute semantics

The export answers "which routines may execute when this routine executes". That is
the engine's call edges plus four rules that model how BC and its test framework run
code. They live in the export (al-sem), not in the consumer, because the facts they
need (object membership, extensions, source tables, parts, attributes, declared types)
are engine facts; re-deriving them in TypeScript would duplicate the engine. All four
are exact derivations from parsed declarations, not new dispatch resolution.

1. **Object runs reach the whole object (lifecycle).** Every `Run` edge targets the run
   object's node (`kind: "Object"`), never the entry trigger directly. The object node
   has `Trigger` edges (`via: "lifecycle"`) to: every trigger of the object and of each
   extension of it in the program; for a page, the object nodes of its parts
   (`ObjectNode::page_controls`), every trigger of its `SourceTable` and of that
   table's extensions, that table's synthetic platform events, and its own synthetic
   platform page events; the object nodes the object's actions run through
   `RunObject`, and the `LookupPageId`/`DrillDownPageId` pages of its source table
   (UI and `TestPage` actions open them); for a symbol-only object, its app's
   callbacks hub (4.5). The edge to the entry trigger is `Resolved`
   (a codeunit run always runs `OnRun`); the others are `OverApprox`. Empty runs use
   `SiteFacts::run_target` (fact 3). Why: a page run fires `OnAfterGetRecord`, field
   and action triggers and its record's triggers; a report run fires its dataitem and
   request-page triggers. Targeting only the entry trigger would report changes to
   those triggers as uncoverable.
2. **Declared runnable variables.** A routine that declares a parameter or local of
   type `Page`, `Report`, `XmlPort`, `Query`, `TestPage` or `TestRequestPage` X (or
   whose object declares such a global) gets a `Run` edge (`OverApprox`,
   `via: "declaredRunnable"`) to X's object node; a `TestRequestPage` maps to its
   report. Why: `TestPage.OpenEdit()`, `TestField.SetValue()`, `TestAction.Invoke()`,
   `ReportVar.SaveAsPdf()` and `QueryVar.Open()` all resolve as catalog builtins, yet
   they run X's triggers. Codeunit variables are excluded: the engine already models
   `CuVar.Run()` precisely, and codeunit variables mostly serve procedure calls.
3. **Test handlers.** A routine with `[HandlerFunctions('A,B')]` gets an `Event` edge
   (`OverApprox`, `via: "testHandler"`) to each named handler in the same codeunit
   (names are case-insensitive). The test framework calls a handler when the matching
   UI event happens during that test, so a changed handler must select the tests that
   list it. Today it is reported uncoverable.
4. **Test runner.** Each `[Test]` method gets a `Run` edge (`Resolved`,
   `via: "testRunner"`) to its own codeunit's object node, because running a test runs
   the test codeunit's `OnRun` first.

### 4.5 Soundness: open sites, hubs and hidden callers

The rule: **every way a routine can run is an edge, a hub edge, or a hidden-callers
flag on the routine.** Then the consumer's backwards walk is sound with one extra rule
(the universal hub) and one policy (hidden callers).

**Open-site bounds come from the resolver.** `SiteFacts` (the resolver's side channel
for per-site facts, `full.rs:127-149`) gets one additive field, `open:
Option<OpenBound>`, set where the facts exist (`resolve_call_site_obligation`):

| `OpenBound` | Set for | Hub | Hub expands to |
|---|---|---|---|
| `Name { name_lc, arity }` | an unresolved bare or member call; a `DynamicOpen` (Variant) member call; an `Unknown` route inside an otherwise resolved call; in each case only when the member name is not a record operation or a run (next rows) | `h:name:<name>/<arity>` | every procedure in the program with that name and parameter count (AL procedures have no optional parameters) |
| `Run { kind }` | an `ObjectRun` with a runtime target (a bare integer id included, fact 3); a dispatching builtin that runs an object (`PageInstance::run`, `ReportInstance::saveas*`, `Session.StartSession`, `TaskScheduler.CreateTask`, page background tasks, ...); an unresolved or `DynamicOpen` member call named `run`/`runmodal` (`kind: any`) | `h:run:<kind>` | every object node of that kind (`any`: every runnable object node) |
| `Record { op }` | a record operation whose table is unresolved (fact 8); `RecordRef` insert/modify/delete/rename and `FieldRef` validate; an unresolved or `DynamicOpen` member call whose name is a record operation (`record_ops::record_op_type`), because the extractor classifies a record operation only on a known record variable (`extract.rs:372-380`), so `X.Insert(true)` on a call result, a Dictionary value or a Variant arrives as a member call | `h:record:<op>` | every `On<Op>`/`OnBefore<Op>`/`OnAfter<Op>` trigger of every table and table extension, and every synthetic platform table event for that op |
| `Any` | a call whose shape names nothing (`UnclassifiedCallee`) | `h:any` (universal) | nothing: see below |

The dispatching-builtin table lives next to `ENTRY_DISPATCH_BUILTIN_IDS` in
`member_catalog.rs`, one entry per builtin id with its bound. The field changes no
edge, histogram or golden.

**Opaque bodies (symbol-only code).** Every symbol-only routine node and symbol-only
object node gets an `Event` edge (`Open`, `via: "opaqueBody"`) to the callbacks hub
of its app, `h:callbacks:<app guid>`. Publishers are excluded, synthetic platform
publishers (`PublisherKind::Platform`, which carry the table's tier) included: a
publisher's body is only "raise", which its `EventFlow` edge already models, and an
opaque-body edge from it would close a hub -> publisher -> hub cycle that marks the
publisher -> subscriber path `dynamic` even where a precise lifecycle or record-hub
edge reaches it. That hub expands, for the app itself and every app B in its own
dependency closure, to: B's publishers that have a subscriber in the program
(synthetic platform events on B's tables and pages included); the public procedures of
program codeunits that implement an interface declared in B; and every trigger of
program extensions of B's objects. That is everything a dependency can call back into
through what its symbols expose (a dependency cannot reference a dependent's objects).

**The universal hub `h:any`** has `universal: true` and no outgoing edges. It stands
for "may call anything": **the consumer treats it as a caller of every node.** Edges
into it (`Open`, `via: "unresolved"` or `"parseRecovered"`) come from `Any`-bound
sites, every routine in a file whose parse was `Recovered` or whose declaration is
`parse_incomplete` (its call sites may be missing), every publisher whose event flow
holds an `Unknown` route, and every routine with an obligation missing from the
coverage contract. A contract failure also sets `meta.coverageHolds = false` and
prints a warning on stderr: an engine bug must be loud, but it never silently drops
reachability.

**Hidden callers** (`hiddenCallers` on a node), for channels no edge or hub covers yet:

| Value | Node | Closed by |
|---|---|---|
| `unraisedPublisher` | (a) every synthetic platform **table** event publisher, unconditionally: record operations do not raise it (fact 9), and hub or lifecycle edges into it must not clear the flag; (b) a real publisher with subscribers but no incoming edge after all rules above. Synthetic **page** events are not flagged: a page's events fire exactly when the page runs, which its lifecycle edges show. | F1 removes (a) |
| `unboundSubscriber` | an `[EventSubscriber]` with no incoming `Event` edge | (a resolution fix per case) |
| `unmodeledRecordTrigger` | a table-extension `OnBefore<Op>`/`OnAfter<Op>` trigger or field `OnBeforeValidate`/`OnAfterValidate` | F5 |

**Consumer policy** (section 8): walk backwards from the changed node(s). A path that
uses an `Open` edge ranks its test `dynamic`. If the reverse cone contains a node with
`hiddenCallers`, static reachability is incomplete for that change: with coverage
history for it, select static ∪ history and warn; without, `fallbackToAll` with a
reason naming the node and channel. A change whose cone has no test, no hidden
callers and no history is `uncoverable`, and with al-sem that is now a proof.

**Stated assumptions** (in `meta.assumptions`; the consumer prints them once in
`warnings` when the selection depended on them):

- `opaqueCodeRunsNoObjectsById`: symbol-only code is assumed not to run program
  objects by a runtime id (`Codeunit.Run(SomeId)`). Assuming it might would connect
  every test that calls any symbol-only app (the platform `System` app included) to
  every object in the program, and static selection would be useless. Dynamic runs in
  dependency code **with** source are modelled (run hubs). Coverage history is the
  backstop; source-shipping dependencies remove the assumption.
- `platformEntryPointsNotModelled`: code the platform starts on its own (session and
  company initialization, global triggers) has no caller, so a change reachable only
  from it is reported uncoverable. F8 adds a catalog of these entry points and closes
  this.

### 4.6 Dependencies, symbol-only apps and pruning

- The export always builds the program with `BuildProfile::FULL` and resolves
  dependency bodies, through the same composition as `alsem analyze`'s cross-app
  model (factored out so the two cannot drift; dependency cache included). There is no
  `--single-app` mode: it would turn every dependency into an opaque body and lose
  precision for no measured need.
- Source-shipping dependencies (embedded or local source): full routine nodes and body
  edges, `dependency: true`, `sourcePath` relative to **that app's** source root (fact
  4), `line`/`endLine`.
- Symbol-only dependencies: see section 6.
- A declared dependency that is missing: calls into it are `Unknown`
  (`ObjectNotInGraph`) and become name hubs; `meta.apps` lists it with
  `source: "missing"`.
- **Pruning.** The output holds every workspace routine (the join needs all of them),
  plus every other node and edge that lies on a path from a workspace routine to a
  workspace routine: the intersection of the forward closure from workspace routines
  and the backward closure to them, computed over all edges and hub edges. Two rules
  make that intersection sound:
  - **The universal hub is a predecessor of every node.** `h:any` has no outgoing
    edges in the file, but the consumer treats it as a caller of every node, so the
    pruning does too: `h:any` is kept whenever any edge enters it, and every node in
    the backward closure counts as reachable from it.
  - **Hidden-callers nodes are kept when they are in the backward closure**, even
    when no forward path reaches them. A node with `hiddenCallers` is, by definition,
    a node whose callers the graph does not show, so it is usually NOT in the forward
    closure (an `unraisedPublisher` synthetic table publisher on a dependency table,
    the common `OnAfterInsertEvent` on a Base App table, has no incoming edge at all).
    Dropping it would leave the subscriber's `Event` edge dangling, the subscriber's
    reverse cone without the flag, and the consumer would report the change
    `uncoverable`, which 4.5 calls a proof. So every node of the backward closure
    that carries `hiddenCallers` is kept, with the edges from it that stay inside
    the kept set.
  This loses nothing for test selection (changed routines and tests are both
  workspace routines) and drops the dependency code that can never call back into the
  workspace, which is most of it (fact 10).

### 4.7 Command

```
alsem graph <workspace> [--out <file>]
```

- JSONL to stdout, or to `--out`. The output carries no timestamp or path, so it is
  always deterministic; there is no `--deterministic` flag (built: it would have
  had nothing to pin). `meta.engine` is `driver_version()`, which
  `ALCH_DRIVER_VERSION_OVERRIDE` pins for goldens and recorded fixtures.
- Exit 0 on success; 1 on any failure, message on stderr (the `alsem query`
  convention, `src/bin/alsem.rs:690-735`).
- Output order is deterministic: `meta`, then nodes sorted by id, then edges sorted by
  `(caller, callee, kind)`.
- No `callers-of` query: the consumer already walks the graph, and a second surface
  would need its own contract.

## 5. The wire contract, v1

**`meta`** (first line):

```json
{"t":"meta","v":{
  "schema":"al-sem.callgraph","version":1,
  "engine":"<driver_version()>",
  "semantics":"may-execute",
  "workspace":{"appId":"…","name":"…","publisher":"…","version":"…","runtime":"17.0"},
  "apps":[{"appId":"…","name":"…","publisher":"…","version":"…","source":"workspace|embedded|localVerified|localApproximate|symbolOnly|missing"}],
  "assumptions":["opaqueCodeRunsNoObjectsById","platformEntryPointsNotModelled"],
  "recoveredFiles":["<app name>::<path>"],
  "coverageHolds":true,
  "counts":{"nodes":0,"edges":0,"hubs":0,"universalCallers":0},
  "histogram":{"primaryScoped":{…},"wholeProgram":{…},"dependencyBodies":{…}},
  "methodIdCalibration":{"enabled":true,"compared":0,"mismatches":0,"unsupported":0,"samples":[]}
}}
```

`histogram` uses the `--program-call-graph-stats` JSON keys. `methodIdCalibration`
arrives with Stage C.

**`node`**:

| Field | Type | Notes |
|---|---|---|
| `id` | string | opaque (4.1) |
| `kind` | `Method` \| `Trigger` \| `Object` \| `Hub` | |
| `appId` | string | app guid, lower case (not on hubs) |
| `objectType` | string | `Codeunit`, `Table`, `PageExtension`, ... (`object_kind_str`) |
| `objectId` | number | omitted for objects without a number (interfaces, ...) |
| `objectName` | string | as declared |
| `memberName` | string | routine name as declared (`ApplyDecay`, `OnValidate`); not on `Object`/`Hub` |
| `member` | string | enclosing field/action/dataitem/control, when there is one |
| `arity` | number | parameter count |
| `sourcePath` | string | relative to the node's own app source root; omitted for symbol-only nodes, objects and hubs |
| `line`, `endLine` | number | 1-based; `line` = declaration keyword or access modifier row, attributes excluded |
| `dependency` | bool | app is not the workspace app |
| `symbolOnly` | bool | no source (its body is opaque) |
| `test` | bool | present and `true` on `[Test]` methods |
| `methodId` | number \| null | Stage C (4.1) |
| `hiddenCallers` | string[] | 4.5; omitted when empty |
| `hub`, `hubArg`, `universal` | | hubs only: class (`any`/`name`/`run`/`record`/`callbacks`), its argument, and `universal: true` on `h:any` |

**`edge`**:

| Field | Type | Notes |
|---|---|---|
| `caller`, `callee` | string | node ids |
| `kind` | `Direct` \| `Event` \| `Interface` \| `Run` \| `Trigger` | 4.3 |
| `confidence` | `Resolved` \| `OverApprox` \| `Open` | 4.3 |
| `via` | string | provenance: `call`, `run`, `implicitTrigger`, `eventFlow`, `interface`, `lifecycle`, `declaredRunnable`, `testHandler`, `testRunner`, `opaqueBody`, `unresolved`, `dynamic`, `dispatchingBuiltin`, `parseRecovered`, `hubExpansion` |
| `line` | number | first call-site line in the caller's file, for engine edges |
| `conditions` | string[] | `manualBinding`, `ambiguousDispatch`, `runTriggerGuarded`, ...; omitted when empty |

**Example**, the D4 chain (ids shortened):

```json
{"t":"node","v":{"id":"r:9f1c…","kind":"Method","appId":"c8becba3-…","objectType":"Codeunit","objectId":50926,"objectName":"Acc Nightly Recalc","memberName":"ApplyDecay","arity":1,"sourcePath":"src/AccNightlyRecalc.Codeunit.al","line":11,"endLine":14,"dependency":false,"symbolOnly":false,"methodId":-612249473}}
{"t":"node","v":{"id":"r:4a07…","kind":"Trigger","appId":"c8becba3-…","objectType":"Codeunit","objectId":50926,"objectName":"Acc Nightly Recalc","memberName":"OnRun","arity":0,"sourcePath":"src/AccNightlyRecalc.Codeunit.al","line":3,"endLine":9,"dependency":false,"symbolOnly":false,"methodId":null}}
{"t":"node","v":{"id":"o:77e2…","kind":"Object","appId":"c8becba3-…","objectType":"Codeunit","objectId":50926,"objectName":"Acc Nightly Recalc","dependency":false,"symbolOnly":false}}
{"t":"edge","v":{"caller":"r:<RecalcNightly>","callee":"o:77e2…","kind":"Run","confidence":"Resolved","via":"run","line":18}}
{"t":"edge","v":{"caller":"o:77e2…","callee":"r:4a07…","kind":"Trigger","confidence":"Resolved","via":"lifecycle"}}
{"t":"edge","v":{"caller":"r:4a07…","callee":"r:9f1c…","kind":"Direct","confidence":"Resolved","via":"call","line":8}}
{"t":"edge","v":{"caller":"r:<NightlyRecalcDecays>","callee":"r:<RecalcNightly>","kind":"Direct","confidence":"Resolved","via":"call","line":46}}
```

Walking back from `ApplyDecay`: `OnRun` (Direct) <- object 50926 (Trigger) <-
`RecalcNightly` (Run) <- `NightlyRecalcDecays` (Direct). The path has non-`Direct`
edges and no `Open` edge, so the test is selected with reason `event`.

## 6. Symbol-only behaviour (the statement #63 asks for)

A symbol-only dependency contributes nodes for the routines the program calls
(`symbolOnly: true`, no `sourcePath`, `methodId` from its `SymbolReference.json` when
listed), for its objects the program runs, and for its publishers that program code
subscribes to. Its bodies are unknown, so each of those nodes has an `Open` edge to the
app's callbacks hub, which reaches everything the app can call back into through what
its symbols expose: subscribers to events published in its dependency closure,
implementations of interfaces declared there, and triggers of program extensions of
its objects. Tests found only through such a hub are ranked `dynamic`, and the
consumer warns which symbol-only apps limited the precision. Runs of program objects
by runtime id inside symbol-only code are not modelled (assumption
`opaqueCodeRunsNoObjectsById`); coverage history is the backstop. Shipping the
dependency with source replaces all of this with precise edges.

**Limit: tests in a separate test app (F7).** v1 works when the tests and the code
they test are in one program as the workspace sees it: one app, or a test folder
nested under the app folder (the engine folds a nested `app.json` folder into the
primary app, `source_text.rs` `NestedApps::Walk`, so its nodes carry the main app's
`appId`). The common layout of sibling `App/` and `Test/` apps does not work yet:

- pointed at the test app, every changed routine lives in the main app, which is then
  a dependency (`dependency: true`), and the consumer's join only considers workspace
  nodes and only discovers files in the project folder;
- pointed at the main app, the test codeunits are in a dependent app, which is not
  part of the program (section 1), so no `[Test]` node exists.

The consumer detects both cases (no node with `test: true`, or every changed file
outside the graph's workspace) and falls back to the whole suite with a reason naming
this limit; it never reports such a change `uncoverable`. The `bcdev_test_select`
description states the limit. F7 (section 9) removes it and is scheduled right after
Stage B, before the selection is described as general.

## 7. al-sem changes

**Stage A: the export** (satisfies #63 with Stage B).

- A1. `SiteFacts::open: Option<OpenBound>` and the dispatching-builtin table (4.5),
  set in `resolve_call_site_obligation`. Additive: no edge, histogram or golden moves.
- A2. Factor `alsem analyze`'s cross-app composition (program build with coverage,
  dependency bodies, merged edges and site facts:
  `program_calls.rs:799-835`) into one function that both call. The shared function
  ends at the merged edges, site facts and parens-less calls. It does **not** include
  `cross_app_demand` (S8.2), which keeps only what the workspace can reach plus the
  code around the dependency events it subscribes to: applied before the export, it
  would remove dependency routines reachable only through interface implementations,
  extension triggers or run hubs, and the callbacks-hub and lifecycle expansions (4.4,
  4.5) would have nothing to expand to. `alsem analyze` applies the demand filter
  after the shared function; the export never applies it and prunes with its own rule
  (4.6) instead.
- A3. New module `src/program/impact_graph.rs` (split into a directory if it passes
  about 800 lines): a pure projection from (graph, decl surface, workspace and
  dependency IR, edges, site facts, snapshot) to rows, implementing 4.3-4.6; pruning;
  the JSONL writer. The module doc is the contract: it restates sections 4.2-4.5 and 5.
  `object_kind_str` in `graphify_export.rs` becomes `pub(crate)`; the graphify export
  is otherwise untouched (BC-Brain reads it).
- A4. `Commands::Graph(GraphCli)` in `src/bin/alsem.rs` (4.7).
- A5. The new fixture `tests/fixtures/test-selection-graph/` (section 10) and tests.
- A6. CHANGELOG `Added`: `alsem graph`. CLAUDE.md (Running, Key Modules) needs a
  one-line mention each; the autonomous flow may not edit CLAUDE.md, so leave that to
  the user if the flow builds this.
- A7. Measure on DO (section 13) and record the numbers in the CHANGELOG entry.

**Stage C: BC methodId** (same design, its own PR).

- C1. ABI ingestion carries a method's `Id` from `SymbolReference.json` to its routine
  (ABI routines only; store it where the S10 compact-graph doctrine puts per-routine
  ABI facts).
- C2. `src/program/method_id.rs`: the port of `calculateProcedureMethodId` and its
  helpers (`al-procedures.ts:162-261`, `:989-1099`): the `NavTypeKind` table, the
  return-value mask, the subtype rules (including List/Dictionary at runtime >= 11
  and the refusal below runtime 7), `[TryFunction]`'s implicit Boolean, event-like
  attributes, the system-codeunit adjustment (ids >= 2,000,000,000), and the
  length-preserving upper-case mapping (`upperInvariantUtf16`: per UTF-16 unit, keep
  the unit if its upper case is not exactly one unit; Rust's `to_uppercase` expands
  `ß`, which gives a wrong id). Inputs come from a type descriptor built two ways:
  from an ABI `TypeDefinition` (calibration) and from source type text, with
  subtypes resolved through the program graph (workspace routines). Any unresolved or
  unmodelled shape gives `None`. Runtime comes from the workspace `app.json`
  (`compilation.runtime`).
- C3. **Calibration** on every run: compute the id of every ABI method in the closure
  that has an `Id` and a supported shape, under its own app's `RuntimeVersion` (top
  of its `SymbolReference.json`), and compare. Any mismatch disables computed
  methodIds for the whole run (all `null`), and `meta.methodIdCalibration` reports the
  counts and up to five samples. Computed ids are also `null` when the workspace
  runtime selects a rule branch no calibrated method exercised (the hash has runtime
  branches below 7 and from 11). Verbatim ABI ids are not affected.
- C4. The export emits `methodId` (4.1); triggers stay `null`.

## 8. bc-dev-mcp changes

**Stage B: adoption** (satisfies #63 with Stage A).

- B1. **Engine choice.** New env vars: `BC_DEV_CALLGRAPH_ENGINE` = `auto` (default)
  | `al-sem` | `altool`, and `BC_DEV_ALSEM` (path to `alsem`; else `alsem` on PATH).
  `createCallGraphProvider(env, run)` replaces the altool-only factory in
  `src/mcp/index.ts:20`. `auto` tries al-sem; if the binary is not found it uses
  altool. If al-sem runs but fails, or writes a `meta` it does not accept, `auto`
  uses altool and adds a warning, and `al-sem` raises a `CONFIGURATION_ERROR`. The
  al-sem provider runs `alsem graph <project> --out <tmp>` with the same timeout and
  error shaping as the altool one. `CallGraph` gains `engine: "al-sem" | "altool"` and
  the parsed `meta`; the tool output reports the engine and its version.
- B2. **Parser.** For al-sem output: require `meta` on the first line with
  `schema = "al-sem.callgraph"` and `version = 1`; read the new node and edge fields;
  apply the fail-safe rules of 4.2. Mark the format with `// WIRE:` comments citing
  this spec and `meta.version`, as the repo does for every wire assumption. The altool
  path is unchanged.
- B3. **Join** (al-sem graphs only). For a changed procedure or trigger: candidates =
  workspace nodes (`dependency: false`, kind `Method` or `Trigger`) with the same
  `sourcePath` and `startLine <= line <= endLine`, kept when `memberName` equals the
  discovered name (case-insensitive). Paths are compared with `/` separators, and
  always case-insensitively on win32: the engine's `sourcePath` comes from the on-disk
  walk (case kept, `source_text.rs:55`) while the consumer's `relativeFile` for a
  changed file is git's recorded path, and the two can differ in case after a rename.
  Names are compared after the same unescaping on both sides: the consumer's
  tokenizer turns a doubled quote `""` inside a quoted identifier into `"`
  (`al-procedures.ts:546`), the engine's `ident_text` only strips the outer quotes
  (`lower/mod.rs:2686-2694`), so the consumer unescapes the node's `memberName` the
  same way before comparing. No candidate, or a name mismatch: fall back to every node of that
  object with that name, with a warning; still nothing: "not in the call graph", as
  today. `TestMethodIdentity` gains `relativeFile`, `startLine` and `endLine` (the same
  discovery has them) so tests join the same way; a test node without `test: true` is
  a warning.
- B4. **Triggers.** With an al-sem graph a changed trigger is joined like a procedure
  and no longer forces `fallbackToAll`; a trigger with no node still falls back. The
  graph is built when any procedure **or trigger** changed (`test-tools.ts:262`).
  `ChangedProcedureRef` gains `kind: "procedure" | "trigger"`.
- B5. **Walk.** `reverseReachable` treats a node with `universal: true` as a caller of
  every node, and returns three classes: `direct` (a path of `Direct` edges only),
  `event` (a path with no `Open` edge), `dynamic` (every path uses an `Open` edge).
  New selection reason `dynamic`, score 0.3, below `event` (0.5). Static agreement
  with history (0.95) applies to `direct` and `event` only.
- B6. **Soundness policy** (4.5): hidden callers in a change's reverse cone mean static
  ∪ history plus a warning when history covers the change, else `fallbackToAll` with a
  reason that names the node and the channel. A changed file listed in
  `meta.recoveredFiles` is a fallback reason. With altool graphs nothing changes, except
  a warning that `uncoverable` is not proven there.
- B7. **Warnings**: `meta.assumptions` (once, when the selection depended on them);
  the symbol-only apps whose callbacks hubs a selection crossed; join fallbacks.
- B8. Docs: README env-var table and the `bcdev_test_select` description (it names
  altool today, and must state the separate test-app limit of section 6); `scripts/e2e.md` gets the al-sem live check (section 12); the
  acceptance fixture README gets the al-sem results.

**Stage C: methodId.** For each joined node: when both the discovered and the node
`methodId` are non-null and differ, warn ("identity disagreement") and look history up
under both. When only the node has one, use it for history. Never drop a test because
of a methodId.

## 9. Engine gaps found (follow-up issues)

Each is a separate issue with its own golden triage and CDO measurement. v1 stays
sound without them; each one only makes selection more precise.

| Id | Gap | Effect today | v1 signal | Closes when |
|---|---|---|---|---|
| F1 | Record operations do not raise platform table events (fact 9) | LSP, detectors and the export miss `OnAfter*Event` subscribers | `unraisedPublisher` on every synthetic table publisher | the resolver adds routes from each record operation on T to T's synthetic publishers for that op (the resolver knows T at `full.rs:700-723`); then rule (a) of `unraisedPublisher` is deleted |
| F3 | Dispatching builtins with a static target (`Report.SaveAsPdf(Report::X, …)`, `Session.StartSession(…, Codeunit::X)`) are catalog calls | selection is coarse (every object of the kind) | `Run{kind}` hubs | the resolver emits `Run` routes for a static first argument (the T0.3 audit already proves such targets) |
| F5 | Table-extension `OnBefore<Op>`/`OnAfter<Op>` and field `OnBefore/OnAfterValidate` are never reached (fact 9) | silent miss in LSP and detectors | `unmodeledRecordTrigger` | `resolve_implicit_trigger` looks them up too |
| F6 | Trigger methodIds are unknown (fact 7) | no coverage history for triggers | `methodId: null` | a rule is derived from compiler vectors (altool node ids carry trigger methodIds; `compiler_oracle.rs` already reads altool graphs) and reproduces 100% of them |
| F7 | Tests in a separate test app (section 6) | pointed at the test app, the changed code is a dependency the join ignores; pointed at the main app, no test node exists | the consumer falls back to the whole suite with a reason (no `test: true` node, or changed files outside the workspace) | scheduled right after Stage B: the engine loads sibling local apps' source (local source providers, `SnapshotBuilder::local_providers`) so `alsem graph <test app>` has the main app's source nodes with their own `sourcePath`; `meta.apps` gains each local app's source root; the consumer discovers across those roots and joins on `(appId, sourcePath)` |
| F8 | Platform entry points (session/company initialization, global triggers) | a change reachable only from them is reported uncoverable | `platformEntryPointsNotModelled` | a catalog of platform entry points, wired as a session-wide pseudo caller (a change it reaches affects every test) |

## 10. Test plan: al-sem

Every new test gets a discrimination proof: break the code with Edit, watch the test
fail, revert with Edit, watch it pass, and record both outcomes (the
`discrimination-proof` skill).

1. **Unit tests** in `impact_graph.rs` over in-memory graphs (the pattern of
   `graphify_export`'s `fixture()`), hand-stating every precondition:
   - one edge of each `EdgeKind` x shape maps to the right `kind`/`confidence`; a
     `Multicast` `ImplicitTrigger` gives `Trigger`, not `Event` (pins the map order);
     a `Call` route to a trigger becomes `Run` to the object node;
   - lifecycle: a page with an extension, a part and a `SourceTable` reaches all of
     their triggers; a codeunit object reaches `OnRun` with `Resolved`;
   - declared runnables, `[HandlerFunctions]` (case-insensitive) and the test runner;
   - each `OpenBound` gives the right hub and expansion; a symbol-only routine gets the
     callbacks-hub edge, and that hub reaches a subscriber of the app's publisher;
   - the universal hub: `Any` sites, recovered files and coverage-contract misses;
   - `hiddenCallers`: a synthetic table publisher is `unraisedPublisher` even when a
     record hub or a page lifecycle has an edge into it; a synthetic **page**
     publisher reached by a lifecycle is not; an unbound subscriber;
   - pruning keeps a workspace -> dependency -> workspace path and drops a dependency
     branch that never returns; every workspace routine survives; a dependency
     `hiddenCallers` node with no incoming edge whose subscriber is a workspace routine
     survives with its flag and its `Event` edge; `h:any` survives when an edge enters
     it and keeps every node of the backward closure;
   - a symbol-only publisher (synthetic platform publisher included) gets no
     opaque-body edge; the callbacks hub of an app expands to that app's own
     publishers too;
   - dedupe keeps the strongest confidence; no self-edges;
   - ids are stable across two runs and distinct for names that differ only in where a
     `/` falls (`"a/b"."c"` vs `"a"."b/c"`); `line`/`endLine` are 1-based.
2. **Resolver tests** for `SiteFacts::open`, one per row of the 4.5 table: an
   unresolved-table record operation gets `Record{op}`, while a resolved table without
   triggers gets nothing (the case fact 8 says the edge cannot tell apart); an
   unresolved member `X.Insert(true)` on an untracked receiver gets `Record{insert}`
   and `X.Run()` gets `Run{any}`, not `Name`; `Codeunit.Run(50000)` gets
   `Run{codeunit}`. Plus the `Call`-to-trigger pin (section 15): a hand-built graph
   where a procedure and a trigger share a name, asserting the `Call` route picks the
   procedure.
3. **Fixture** `tests/fixtures/test-selection-graph/`: a trimmed copy of bc-dev-mcp's
   acceptance fixture (the Loyalty event + `Codeunit.Run` chain, the Tax interface with
   three implementations, the Invoice event, Legacy Export, one test codeunit per
   module) plus: a page with a `SourceTable`, `OnAfterGetRecord`, a field `OnValidate`
   and an action, a `TestPage` test and a `ModalPageHandler` test; a table with
   `OnInsert` and a subscriber to its `OnAfterInsertEvent`; a dynamic
   `Codeunit.Run(CodeunitId)`; a Variant member call; a `RecordRef.Insert(true)`; a
   `Report.SaveAsXml(Report::X, …)`; an unbound subscriber.
4. **CLI member** `tests/cli/cli_graph_export.rs` (cli umbrella; harness modelled on
   `cli_query_differential.rs`): runs the `alsem` binary on the fixture and walks the
   output backwards with a small walker that mirrors the consumer's rules (universal
   hub, `Open` -> dynamic). It asserts:
   - `ApplyDecay` and `OnRun` reach `NightlyRecalcDecays`, through a `Run` edge;
   - `TaxAmount` has three `Interface`/`OverApprox` edges, one per `RatePct`;
     `OnAfterInvoiceTotal -> CashRoundOnInvoiceTotal` and
     `OnAfterCalcPoints -> AddBonusOnCalcPoints` are `Event` edges (`via: eventFlow`);
   - `ExportLine` reaches no test and its cone has no hidden callers;
   - the page triggers reach the `TestPage` test and the handler test; the table
     trigger reaches the test that inserts the record;
   - the subscriber to `OnAfterInsertEvent` carries `unraisedPublisher` in its cone;
   - the dynamic sites reach their hubs, and the tests that call them reach the hub
     targets as `dynamic`;
   - every workspace node has `sourcePath`, `line` and `endLine`, equal to hand-stated
     expected rows (not computed by production code).
5. **Cross-app member** (temp dir, `tests/common/symbol_app.rs::write_source_app`, as
   `tests/r4/r4_cross_app_program.rs` does): a source dependency whose code raises an
   event that a workspace subscriber handles, called from a workspace test; a
   dependency table that the workspace extends (an `OnInsert` edge, and a table
   extension `OnAfterInsert` flagged `unmodeledRecordTrigger`); a symbol-only
   dependency with an ABI publisher that a workspace subscriber handles, and a routine
   a workspace test calls; a workspace subscriber to a **dependency** table's
   `OnAfterInsertEvent`. Asserts dependency nodes (`dependency: true`, `sourcePath`
   relative to the dependency, lines), symbol-only nodes with no `sourcePath`, that the
   symbol-only subscriber reaches its test only through the callbacks hub (`Open`), and
   that the dependency table's synthetic publisher survives pruning with
   `unraisedPublisher` and its `Event` edge to the workspace subscriber.
6. **Byte golden** `tests/cli-query-goldens/graph.test-selection-graph.jsonl`
   (`ALCH_DRIVER_VERSION_OVERRIDE` pinned; a directory `--test cli` already owns, so
   `scripts/` is not touched). Pins the format.
7. **Required discrimination proofs** (at least): map `Run` to `Direct` (the D4 class
   assertion fails); drop the lifecycle closure (page-trigger reachability fails); put
   shape before kind in the map (the `Trigger` unit test fails); drop the universal
   hub rule (the `Any`-site soundness test fails); drop `SiteFacts::open` for
   unresolved tables (the record-hub test fails); flag synthetic table publishers only
   when they have no incoming edge (the hub-reached `unraisedPublisher` test fails);
   drop the dependency-body edges (the cross-app test fails); drop the `+1` on `line`
   (the row assertion fails).
8. **Stage C**: unit vectors from bc-dev-mcp's `tests/fixtures/coverage-gap/`
   (`compiler-method-ids.json`, `test-method-ids.json`, including `Größe`) and from the
   acceptance fixture's `coverage-map.json` (`ApplyDecay` `-612249473`, ...); the three
   Business Foundation `GetNextNo` vectors copied into a hand-written
   `SymbolReference.json`; ABI passthrough; calibration with one deliberately wrong
   `Id` (every computed id becomes `null`, `meta` reports it). A CDO-gated test (the
   `tests/common/cdo.rs` gate, run by `scripts/cdo-gate` at the branch gate) computes
   the id of every embedded-source dependency routine from its **source** and compares
   it with its own symbol file's `Id`.
9. **Gates**: `rustfmt` per touched file; `cargo test -p al-sem --lib impact_graph`;
   `cargo test --test cli cli_graph_export::`; `scripts/check-goldens` (only the new
   golden appears; nothing else may move); `scripts/ci-steps task`; `scripts/ci-steps
   all` before merge.

## 11. Test plan: bc-dev-mcp

Same discrimination-proof rule, with `bun test` and `bun run typecheck`.

1. `tests/core/call-graph.test.ts`: v1 parsing (`meta` first, new fields), rejection of
   an unknown major version and of a missing `meta`, the fail-safe rules for unknown
   values, unknown `t` lines ignored, the universal hub in `reverseReachable`, the
   three walk classes.
2. `tests/core/test-selection.test.ts`, with hand-written JSONL graphs: the
   containment join with attributes above the keyword; overloads told apart; two
   `OnValidate` triggers in one table told apart; a name mismatch falls back to the
   name join with a warning; no node gives the existing fallback; hidden callers with
   and without history; a recovered changed file; changed triggers no longer fall
   back; `dynamic` ranks below `event`; a path differing only in case joins (win32);
   a quoted name with a doubled quote joins; a graph with no `test: true` node falls
   back with the separate-test-app reason (section 6).
3. Provider tests (fake `CommandRunner`): `auto` prefers al-sem; al-sem not found
   uses altool; an al-sem failure uses altool with a warning; explicit `al-sem` raises
   the configuration error; the arguments passed to `alsem`.
4. `tests/core/test-selection-acceptance.test.ts`, a new `describe` over
   `call-graph.al-sem.jsonl` (recorded with `alsem graph`, the version pinned by
   `ALCH_DRIVER_VERSION_OVERRIDE`):
   - no coverage map: D1 selects 9, D2 10 (`event`, via `Interface`), D3 5, D4 1
     (`NightlyRecalcDecays`, `event`), D6 5 plus `ExportLine` `uncoverable`, and the
     D5 `OnRun` change selects 1 with `fallbackToAll: false`; every live failure listed
     in `liveDiffs` is selected;
   - with the recorded map: the union, D4 with reasons `event` + `coverage-history`;
   - every procedure and trigger the discovery reports joins to exactly one node.
   The existing altool `describe` stays, so both engines stay covered.
5. A dependency test: `.app` packages synthesized at test time with
   `tests/fixtures/app-package.ts` (one with embedded `src/*.al` entries, one
   symbol-only), plus a recorded al-sem JSONL for offline runs, asserting the
   section 6 behaviour from the consumer's side.
6. A live test, skipped unless `BC_DEV_ALSEM` is set or `alsem` is on PATH: runs the
   provider on the acceptance fixture and checks the structural facts (not bytes,
   since the engine version may differ).

## 12. End-to-end acceptance procedure

1. In al-sem, on the branch: `cargo build --release --bin alsem`; set `BC_DEV_ALSEM`
   to `target/release/alsem.exe`.
2. `ALCH_DRIVER_VERSION_OVERRIDE=<label> alsem graph U:/Git/bc-dev-mcp/tests/fixtures/test-selection-acceptance
   --out <bc-dev-mcp>/tests/fixtures/test-selection-acceptance/call-graph.al-sem.jsonl`.
   Inspect: the D4 chain of section 5, three `Interface` edges from `TaxAmount`, two
   `Event` edges, `ExportLine` with no incoming edge.
3. In bc-dev-mcp: `bun test` and `bun run typecheck` (the offline acceptance of
   section 11.4 is the #63 acceptance in code).
4. Live, on the Cronus28 container, following the fixture README: for each planted
   bug D1-D7, run `bcdev_test_select baseRef=HEAD` **without** `coverageMapFile`
   (engine al-sem) and record the selection; then with the map. Publish the changed
   app, run the full suite and the selected shards with `coverageAgainst=HEAD
   changesDeployed=true`, and confirm no test that failed in the full run was left out.
   Expected without a map: D4 selects `NightlyRecalcDecays`; D5 selects it without
   falling back; D7 still falls back (a non-AL file changed). Record the table in the
   fixture README next to the altool one.
5. Dependencies: copy BC28 symbols into `.alpackages/` (as the README's rebuild step
   does), rerun step 2 into a scratch file, and record the `meta.apps` sources, the
   dependency node counts and any callbacks hubs. Repeat with symbol-only packages and
   record the `dynamic` selections and the warnings they produce.
6. Soundness spot check: plant a change in a routine reachable only through a platform
   table event subscriber (an `OnAfterInsertEvent` subscriber added to a scratch copy
   of the fixture). Expected: `fallbackToAll` without a map, with a reason naming
   `unraisedPublisher`.

## 13. Measurement (Stage A)

On DO (`U:/Git/DO/Cloud`, not the CDO gate) with the `release-fast` `alsem`: wall time
(cold and warm dependency cache), peak memory, output bytes, and counts of nodes,
edges, hubs and universal callers, with and without pruning. Also count `Unknown` and
`HonestDynamic` sites in dependency bodies; those numbers bound how much the universal
and name hubs widen selections. Record the numbers; there is no bound to meet yet. If
the warm run is too slow for interactive use, the next step is caching the graph per
input state in the consumer, not a less precise export.

## 14. Rejected alternatives

- **altool's id format** (`al . <guid> / Codeunit 50926 Name # Member#methodId`): ties
  the contract to an undocumented Microsoft string, and has no id for triggers or
  `null` methodIds.
- **methodId as the only join key**: no triggers, `null` for unresolved signatures,
  name fallback merges overloads.
- **Not emitting methodId** (the investigation's default): the graph join does not
  need it, but coverage history does, and al-sem is the better-validated source.
- **An `openCalls` count on the caller** (the investigation's default for open sites):
  it says a site is open but not what it may reach, so the consumer could only treat
  every open site as reaching everything. Bounded hubs are as simple for the consumer
  and far more precise.
- **Emitting every dependency routine**: about 121k nodes on DO, nearly all off any
  workspace path.
- **Test-selection rules in the consumer** (lifecycle, handlers, test pages): would
  re-derive engine facts in TypeScript.
- **Assuming symbol-only code may run anything**: every test touching any symbol-only
  app (the platform `System` app included) would reach everything.
- **Adding platform-event and table-extension trigger routes in the export**: that is
  dispatch semantics, so it belongs in the resolver for every consumer (F1, F5).
- **`--format json`, `--single-app`, a `callers-of` query**: no consumer needs them.

## 15. For the builder to confirm while building

- That no `Call` route reaches a trigger except through `dispatch_entry_trigger` (4.3
  relies on it). The resolver does not enforce it: `resolve_in_object` and
  `routines_in_object` do not filter on `is_trigger` (every `is_trigger` in
  `resolver.rs` is a test fixture), so the claim rests on AL forbidding direct trigger
  calls. Pin it with a resolver test over a hand-built graph where a procedure and a
  trigger share a name, asserting the `Call` route picks the procedure; otherwise a
  future resolver change silently turns such edges into `Run` edges to the object
  node.
- Whether the IR exposes XmlPort table elements. If it does, add their tables'
  record triggers to the XmlPort lifecycle; if not, list `xmlportAutoSave` in
  `meta.assumptions` and file a follow-up.
- The exact builtin ids of the dispatching-builtin table (grep the catalogs; each id
  gets a resolver test).
- Where the request page's triggers sit in the IR (report object, member or not);
  the lifecycle closure must include them either way.
- Whether the IR exposes action `RunObject` and table `LookupPageId`/`DrillDownPageId`
  values (4.4 rule 1). If one is missing, list `propertyRunsNotModelled` in
  `meta.assumptions` and file a follow-up rather than drop the rule silently.

## 16. Build notes (Stage A, as built)

Answers to section 15 and the decisions the build took:

- **`Call` to a trigger.** The resolver did NOT hold the premise: a field trigger
  `OnValidate` and a procedure `OnValidate()` in one table made a call to the procedure
  an ambiguous two-route edge that included the trigger. `resolve_in_object` now leaves
  triggers out of the call candidates (AL code cannot call a trigger); the pin is
  `full::tests::a_call_never_resolves_to_a_same_named_trigger`.
- **XmlPort table elements** are in the IR (`ObjectDecl::dataitems` for an XmlPort);
  their tables' triggers are in the XmlPort lifecycle, so `xmlportAutoSave` is not
  needed.
- **Dispatching builtins**: `member_catalog::DISPATCHING_BUILTINS`, pinned by
  `dispatching_builtins_are_real_catalog_ids` (every id is one the catalog emits).
  `Record::insert`/`modify`/... are listed too: a record receiver the extractor does
  not classify as a record operation reaches the `Record` catalog.
- **Request-page triggers** are routines of the report object (with or without an
  enclosing member), so "every trigger of the object" covers them.
- **`RunObject`** was not in the IR; the lowerer now collects every `RunObject`
  property value outside code into `ObjectDecl::run_objects`. `CardPageId`,
  `LookupPageId` and `DrillDownPageId` are object properties and were already there.
  `propertyRunsNotModelled` is therefore not needed.
- **Variables of type `Codeunit`** are left out of rule 2, as the rule says.
- **Pruned object nodes.** An object node is kept only on a workspace-to-workspace
  path, so a test codeunit with no `OnRun` loses its `testRunner` edge in the output
  (nothing for the consumer to walk).
- **Additive wire details** the tables above did not name: node `synthetic: true`
  on an engine-synthesized platform event publisher (no declaration, so no
  location); `via: "coverageMiss"` for a coverage-contract miss into `h:any`. Hub
  edge kinds: `name` and `any` hubs use `Direct`, `run` uses `Run`, `record` uses
  `Trigger`, `callbacks` uses `Event` (into the hub) and `Event`/`Interface`/`Trigger`
  (out of it); every hub edge is `Open`.
- **`methodId`** is not emitted until Stage C.
- **Measured on DO** (`U:/Git/DO/Cloud`, release-fast `alsem`, 2026-10-13, dev
  machine; 10 apps with embedded source plus the symbol-only `Application`): 10.4 s
  cold / 9.2 s warm wall, 2.13 GB peak working set, 24.0 MB of JSONL; 31,827 nodes
  (14,730 methods, 14,216 triggers, 2,871 objects, 10 hubs) and 79,860 edges after
  pruning; no universal-hub caller; coverage contract holds. Dependency bodies: 431,321
  call sites, 0 `unknown`, 1,059 `honestDynamic`. The unpruned counts were not
  measured.
