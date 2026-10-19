# Issue #107: an event publisher's parameter roles compose its subscribers

Date: 2026-10-19. Branch `issue/107-agent-discovery-l4-summary-solve-a1`, base `8405f96d`.

## Goal

An event publisher's `RecordRoleSummary` for a record parameter (`persistsCurrentRecord`,
`validatesParam`, `requiresLoadedAtEntry`, `mutatesBeforeLoad`, `dirtyAtExit`, the loaded-field
lists, ...) must include what its subscribers do to that parameter, the same way a routine's roles
already include what its callees do at a call site. Then a routine that raises the event (and so
calls the publisher) sees those effects through the existing call-site composition, and the
detectors that read callee roles see them too.

Today roles come only from the routine's own call sites (c1b, `summary_runner.rs:635-728`) and
its body walk (`cfg_walker::walk_param`, `summary_runner.rs:738-774`). An event-dispatch edge
(publisher -> subscriber) has no call site (`combined_graph.rs:332-340`), so neither step sees
it, and the publisher's body is empty. So a publisher looks like a no-op to every caller.

## Assumption probes

Fixtures and outputs: `U:/Git/al-call-hierarchy/.agent/runs/20261019-071107-16946d/probes/`
(`ws-pub`, `ws-cycle`, `ws-dep-src`, `ws-dep-sym`, built by `gen/mkdep.py`; `aldump` from this
worktree, `cdo` profile).

| probe | result | evidence |
|-------|--------|----------|
| (a) workspace reproducer | **Confirmed.** `OnRecModify(var Rec)` with a subscriber `Rec.Modify()`: the publisher's role is all `no` (`persistsCurrentRecord`, `mutatesParam`, `requiresLoadedAtEntry`, `mutatesBeforeLoad`, `dirtyAtExit` all `no`, loaded `full`). Its caller `ForwardModify(var Rec) { OnRecModify(Rec) }` is all `no` too. Same for `OnRecValidate` (subscriber `Rec.Validate(Name,'x')`, itself `dirtyAtExit: yes`): publisher `dirtyAtExit: no`. | `aldump --r3a2-summary-core probes/ws-pub` -> `a.json`; routine names mapped through `--r3a1-combined-graph` -> `a1.json` (publisher `#d7e9…`, caller `#70fc…`, subscriber `#4340…`) |
| (a') the acceptance's "dirty at exit" | **False as written.** In this engine `Modify` moves a record to `Persisted`, not `DirtyV`, so a DIRECT call to a routine that does `Rec.Modify()` already gives `dirtyAtExit: no` (with `persistsCurrentRecord`, `requiresLoadedAtEntry`, `mutatesBeforeLoad` = `yes`). `dirtyAtExit: yes` comes from `Validate`/field writes. So the correct oracle for "reflects the subscriber's Modify" is: the publisher's role equals what a direct call to the subscriber would give its caller. | control `ControlDirect(var Rec) { DoModify(Rec) }` in `ws-pub`: `yes/yes/yes/yes`, `dirtyAtExit: no`; `cfg_walker.rs` `apply_op` `"persistsCurrent" => Dirty::Persisted`, `compute_dirty_at_exit` counts only `DirtyV` |
| (b1) source-bearing dependency publisher (cross-app) | **Defect confirmed for the body-local facts.** Both dependency publishers (`OnDepRec` with a dependency subscriber `Modify`, `OnDepRec2` with a WORKSPACE subscriber `Modify`) are `fixed_roles` (#70): `persistsCurrentRecord: no`, `mutatesParam: no`, the five path-sensitive fields `unknown`. The workspace forwarders `Fwd`/`Fwd2` inherit `persistsCurrentRecord: no`, `mutatesParam: no` (c1b reads the publisher's `no`; the publisher has `body_available`). A workspace subscriber's effect does NOT reach a dependency publisher. | `aldump --r3a5-cross-app-summary probes/ws-dep-src` -> `ws-dep-src.r3a5.json`; `capability_cone.rs` `build_cross_app_base` (`fixed_roles` = dependency routines with a body and an out-edge; only event dispatch survives as a dependency out-edge) |
| (b2) symbol-only dependency publisher | **No defect for guarded readers; two readers are unguarded (round 1).** The publisher is bodyless (`opaque-body`); its own role says `persistsCurrentRecord: no`. c1b, the walker, d37 and d39 apply the opaque guard: the forwarders are all `unknown` with `opaque-callee`. But `d41` (checks only the CALLER's body, then reads the callee's `resets_filters_on_param`) and `record_load_points` (no `body_available` check) read a bodyless callee's role as given. That gap is pre-existing and applies to every bodyless callee, not only publishers. | `ws-dep-sym.r3a5.json`; c1b `opaque` branch (`summary_runner.rs:680-700`), `cfg_walker::apply_call` `callee_opaque`, d37 `suppress-unknown` on `!body_available` (`d37.rs:251`), d39 skips `!body_available` (`d39.rs:66`); unguarded: `d41.rs:56-95`, `detectors/mod.rs:199-225` |
| (b3) dependency publisher, single-app mode | **Out of reach by construction.** A dependency publisher is not a model routine there (`EventSymbol.publisher_routine_id = None`, `publisher_ref = Dependency`), so no dispatch edge is built and nothing calls it in the model. | `events.rs` `PublisherRef`; `combined_graph.rs:349` `let Some(publisher_routine_id) … else continue` |
| (c) consumers of publisher roles | Roles are read only as CALLEE roles, at a resolved call site, plus d39's per-routine scan: c1b (`summary_runner.rs` compose), the walker (`cfg_walker::apply_call`), `d37-validate-without-persist` (`persists_current_record`), `d39-record-left-dirty-across-chain` (`dirty_at_exit`, plus load points), `d40-transitive-load-missing` (`requires_loaded_at_entry`, `mutates_before_load`, load points), `d41-transitive-filter-loss` (`resets_filters_on_param`), `d42-cross-call-wrong-setloadfields` (`required_loaded_fields_at_entry`, `current_loaded_fields_at_exit`), `detectors/mod.rs` load points (`puts_in_loaded_state`). The trio behind `puts_in_loaded_state` (`loads_from_db_param`, `initialises_param`, `copies_into_param`) is read as a MUST fact in both directions: the walker and d40 drop an entry requirement after it (lenient), and d39 keeps a caller FLAGGED when a load point follows the call (`d39.rs:185-204`: the reload discards the dirt), so a MAY load read as MUST can create a d39 finding. `d3-missing-setloadfields` cannot move: it escapes every `event-publisher` callee (`d3.rs:392-399`) and reads only `may_*` fields, which no composition step touches. Projections: `project_r3a2` (`tests/r3a2-goldens/*`, every file carries `parameterRoles`), `project_r3a5_cross_app` (`tests/r3a5-goldens/`), the roles fixpoint change key. `db_effect_solver` and the capability cone do not read roles (they carry them only). | `grep -rn "parameter_roles\|dirty_at_exit\|requires_loaded_at_entry\|…" src/engine` |
| (d) subscriber -> publisher parameter binding | **By NAME**, case-insensitive, subscriber may declare any subset: `event_param_temp_state` (`summary_runner.rs:906`, trims quotes). A second, inline copy of the same mapping lives in `pd_solve::carry_reqs` (`pd_solve.rs:170-185`) and does NOT trim quotes, so `"Rec"` and `Rec` match in one and not the other. | source read |
| (e) solver order vs dispatch edges | **Subscribers settle first; cycles are already handled.** Every SCC input graph is `graph.edges_by_from`, which holds the event-dispatch edges, so Tarjan puts a subscriber's SCC before a non-recursive publisher's. A subscriber that re-raises its event forms a recursive SCC `{publisher, subscriber}`, and the roles Jacobi's intra-SCC dependents are built from ALL `edges_by_from` edges, dispatch included. | `ws-cycle`: SCCs `[(1, non-recursive) HandleCyc2, (2, recursive) {OnCyc, HandleCyc}]` (`cyc1.json`); `summary.rs:854-863`, `detector_context.rs:1391-1400`, `capability_cone.rs:3534-3542`; `summary_runner.rs:1704-1713` |
| (f1) several subscribers | AL runs every bound subscriber, order not specified. Composition must cover every order. | `ws-cycle`: `OnCyc` dispatches to `HandleCyc` and `HandleCyc2` |
| (f2) unresolved / unknown subscribers | **Round 1 correction: not always resolved.** An `ambiguous`/`maybe`/`unknown` subscription gets a synthesized symbol, but `synth` returns early when the event id already exists (`events.rs:340-342`). An ambiguous subscription to an overloaded publisher (same object, same event name, so the same `encode_event_id`) therefore keeps the REAL symbol's id, and `build_combined_graph` turns it into a dispatch edge with `resolution: "ambiguous"` (it forwards `ee.resolution` unfiltered, `combined_graph.rs:359-376`). Two subscriptions of one subscriber to two overloads can also give a duplicate (publisher, subscriber) edge. A subscriber with no summary, no role for the parameter, or no body is the opaque case. | `events.rs:330-345, 495-510`, `combined_graph.rs:348-376` |
| (f3) subscriber omits the parameter / takes it by value | Omitted (`HandleOmitted(Flag)`): no name match, so the subscriber cannot touch it. By value (`HandleByValue(Rec)` doing `Modify`): its own role is `persists: yes`, `requiresLoadedAtEntry: yes`. The call-site rules already cover both shapes: entry requirements compose whatever the var-ness (c1a), exit effects only when both sides are `var` (c1b). | `a.json` (`#d3db…`), `cfg_walker::apply_call` comments |
| (f4) manual binding / license conditions | `EventSubscriberInstance = Manual`, `SkipOnMissingLicense/Permission` become `Condition`s on the program engine's `Subscription` (`index.rs:123-124`), which `events.rs` already iterates when it builds each `EventEdge` (`s` in `events.rs:395-545`); the model `EventEdge` does not copy them. Such a subscriber MAY not run. | `program/resolve/index.rs:90-124, 1147-1165`, `events.rs:84-92` |
| (f5) publisher bodies are empty | AL requires an event publisher's body to be empty (Microsoft docs, "Publishing events"); 190 event publishers in CDO, 0 with code in the body. | `gen/pubbody.py U:/Git/DO-cdo-baseline/Cloud` |
| (f6) isolated events | `[IntegrationEvent(…, Isolated=true)]` / `[BusinessEvent(…, true)]` is read into `EventSymbol.isolated` (`events.rs:59, 118-136`). An isolated subscriber that errors has its database work rolled back and the publisher continues; its in-memory changes to `var` parameters are not rolled back. | source read |
| (f7) default detector set | `d40-transitive-load-missing` is OPT-IN (`detectors/mod.rs:1548-1558`); `d37`, `d39`, `d41`, `d42` (and `d3`) are default (`detectors/mod.rs:1234-1450`). | source read |
| (f8) quoted parameter names | Source parameter names are already unquoted by the lowerer (`ident_text`, `crates/al-syntax/src/lower/mod.rs:2178-2181, 3321-3329`), so the two mappings of probe (d) can differ only for an ABI-derived name that carries quotes. | source read |

## Non-goals

- Walking a dependency publisher's own body. #70 fixed its path-sensitive role fields at
  `unknown` because its call sites are cut from the combined graph; that stays (they remain
  `unknown`). Only the body-local presence facts of a dependency publisher compose.
- Open-world subscribers (apps outside the model that may subscribe to a public event). The
  model's event graph is closed-world everywhere else (the cone, the db-effect store); roles
  follow it.
- Platform trigger events (`OnBeforeModify` on a table, page events). They have no publisher
  routine, so no dispatch edge and no roles; unchanged.
- Trigger RAISERS. A table trigger that raises `OnBeforeX(Rec, xRec)` passes its implicit
  `Rec`, which is not a record parameter of the trigger: `compute_record_roles` builds roles only
  for declared record parameters, and c1b needs `caller_source_parameter_is_var == Some(true)`
  (`summary_runner.rs:663`). So a trigger gains no role from the publisher, exactly as it gains
  none from any helper it calls with `Rec` today. Unchanged; unit row 9 pins it.
- Receiver flow through `Sender`. On a table event with `IncludeSender`, a subscriber's
  `var Sender: Record T` is the record the event was raised on. That flow is a receiver binding,
  not a by-name parameter binding; it is not modelled (Sender matches no publisher parameter and
  is ignored). Effects a subscriber has on `Sender` are lost, the same as before this change.
- Single-app mode dependency publishers (probe b3): no model routine, no consumer.
- The missing `body_available` guard in `d41` and `record_load_points` (probe b2). It concerns
  every bodyless callee; filed as a discovery. This change keeps it from widening (Design 6).
- Proving the roles transfer monotone. It is not monotone today (Design 5); this change adds a
  guard, not a proof.

## Acceptance matrix

| Acceptance item (issue) | Proof |
|-------------------------|-------|
| "the publisher's roles for Rec reflect the subscriber's Modify ... for workspace publishers" | New summary test (unit, `summary_runner.rs` tests or an `r3a2` fixture `ws-event-roles` + golden). Hand-stated: `OnRecModify`'s role for `Rec` = `persistsCurrentRecord yes, mutatesParam yes, requiresLoadedAtEntry yes, mutatesBeforeLoad yes, dirtyAtExit no` (equal to `ControlDirect`, probe a'); `OnRecValidate`'s `dirtyAtExit yes`; `ForwardModify` / `ForwardValidate` inherit them through c1b/the walker. "dirty at exit" in the issue text is restated per probe (a'): Modify does not dirty a record in this engine. Discrimination: drop the dispatch step -> the test fails with the probe-a output; revert -> passes. |
| "... and dependency publishers alike" | Extend #70's cross-app test (`capability_cone.rs` `issue70_publisher_roles_tests`): `OnDepRec` (dependency subscriber) and a new `OnDepRec2` (WORKSPACE subscriber, fixture of `probes/gen/mkdep.py`) have `persistsCurrentRecord yes`, `mutatesParam yes`; their five path-sensitive fields stay `unknown` (#70 row 3c unchanged); `Fwd`/`Fwd2` inherit `persistsCurrentRecord yes`. Checked through both `ctx.parameter_roles_by_routine` (detector context) and `project_r3a5_cross_app`. Re-run #70's two recorded mutations and record the outcomes, and add a third: "skip the dispatch fold for `fixed_roles` members" -> the new rows fail. A symbol-only dependency publisher (`ws-dep-sym`): its own role and every caller's role are byte-identical to base (Design 6), asserted as row 8. |
| (implied) the new shapes | Unit rows, each hand-stated and each with a discrimination proof (break, watch fail, revert, watch pass): **1** one `Modify` subscriber = ControlDirect; **2** one `Validate` subscriber -> `dirtyAtExit yes`; **3** subscribers `A = Validate; Modify` and `B = Modify` -> `dirtyAtExit no` (every order persists; the round-1 closure gave `yes`); **4** two unconditional `Get` subscribers -> publisher `loadsFromDbParam yes`, walker `loaded` stays `Yes` after the dispatch (adding a no-op subscriber does not change the role); **4b** (mixed `U`/`O`, sol round 3) an unconditional `Get` subscriber plus a MANUAL no-op subscriber -> publisher `loadsFromDbParam yes`, publisher walk `loaded = Yes` after the dispatch (the incoming unloaded state is never an outcome), and a caller `F(var R) { OnMixed(R); R.Modify() }` gets `requiresLoadedAtEntry no`; **5** one MANUAL (`EventSubscriberInstance = Manual`) `Get` subscriber -> publisher `loadsFromDbParam no` (`optional_load`), and a caller `Validate(R); OnLoad(R)` gains no d39 load point; **5b** (sol round 2, through the CALLER) `F(var R) { R.Get('A'); OnOptionalGet(R); R.Modify(); }` with a manual `Get` subscriber -> `F`'s `requiresLoadedAtEntry no` (the earlier `Loaded::Yes` survives the dispatch; with `unknown` it would be `yes`), and its caller `C() { F(X) }` with an unloaded `X` gets no d40 finding from it; **5d** (sol round 3) a sole MANUAL subscriber doing `Rec.Copy(Other)` -> publisher `copiesIntoParam no` (masked), `mutatesParam yes` (unmasked), `dirtyAtExit yes` (run branch `DirtyV`); a forwarding caller `Fwd(var R) { OnCopy(R) }` -> `mutatesParam yes` (c1b joins the callee's `mutates_param`), `copiesIntoParam no`, `dirtyAtExit yes`, no load point; **5c** mixed readings: a subscriber with `SkipOnMissingLicense` in one `#if` reading and none in the other -> `Subscription.conditions` empty (unchanged), `conditional_any` true, edge `conditional`, publisher trio from its `Get` = `no`; **6** an isolated event with a `Validate; Modify; Error` subscriber -> publisher `persistsCurrentRecord yes`, `validatesParam yes` (visible MAY facts), the five path fields `unknown`, `loadsFromDbParam no`; a forwarding caller `Fwd(var R) { OnIso(R) }` gets `dirtyAtExit no` (the walker's persist rule runs before the validate rule, `cfg_walker.rs:1338-1346`; a known leniency for the failed run); **6b** isolated `Validate; Error` subscriber -> publisher `validatesParam yes`, `persistsCurrentRecord no`; `Fwd` gets `dirtyAtExit yes`; **7** event-id collision: an ambiguous subscription to an overloaded publisher (`resolution: "ambiguous"` dispatch edge) -> unseen dispatch: every presence and path fact of `p` `unknown`, no `yes` from any other subscriber of that event; **7b** one resolved and one ambiguous edge to the SAME subscriber -> the `Opaque` copy absorbs the `Exact` one (unseen); **7c** a duplicate resolved (subscriber, q) edge counts once (n = 1, exact); **8** bodyless (symbol-only) publisher -> roles identical to base; **9** a table trigger raising the event -> no role change (non-goal pinned); **10** `ws-cycle` recursive SCC converges (no cap) with `HandleCyc`'s `Modify` in `OnCyc`'s role; **11** `HandleOmitted` adds nothing, `HandleByValue` adds the entry requirement only; **12** five subscribers (over the exact bound), one of them `Get` and one `Validate` -> unseen dispatch: publisher presence and path facts all `unknown` (no `loadsFromDbParam yes`, no `validatesParam yes`), and through a forwarding caller `Fwd(var R) { OnFive(R) }`: no proven load, no `dirtyAtExit yes` from the subscribers, no load point (sol round 2); **12b** (sol/opus round 3) a caller `G(var R) { R.SetLoadFields(Name); OnFive(R); R.Get('A'); X := R."No." }` -> after the unseen dispatch the narrowing is `Unknown` (not kept), so `G`'s loaded fields after the `Get` are `Unknown` (with the narrowing kept they would be `[Name]`); and a caller that validated before the call keeps its own `DirtyV` (`dirtyAtExit yes` from `G`'s own `Validate`, not from the subscribers); **13** a hand-built subscriber whose parameter name is `"Rec"` (quoted, as an ABI name could be) binds to the publisher's `Rec` in `event_publisher_param`, and through `carry_reqs` too (Design 1). |
| (implied) no unrelated movement | `scripts/check-goldens --regen`: every moved golden line is a publisher, a routine that (transitively) calls one, or a finding of d37/d39/d41/d42 at such a call; triage receipt `.agent/golden-triage.md`. `d3` and every non-role detector byte-identical. CDO measurement below. |

## Design

### 1. One parameter mapping

Add in `summary_runner.rs`, next to `event_param_temp_state`:

```rust
/// The publisher parameter an event subscriber's parameter `sub_param` binds to:
/// AL binds by NAME, case-insensitively, quotes ignored; a subscriber may
/// declare any subset.
pub(crate) fn event_publisher_param<'p>(
    publisher: &'p ModelRoutine,
    subscriber: &ModelRoutine,
    sub_param: u32,
) -> Option<&'p ModelParameter>
```

`event_param_temp_state` and `pd_solve::carry_reqs` both call it, so the inline copy in
`carry_reqs` goes away. The new role step uses it too. No other copy of the mapping exists.

**Decision: unify, with quote trimming, and name what that can move.** Keeping two mappings
that disagree is the defect; the root-cause fix is one mapping. `carry_reqs` then trims quotes
where it did not, which can move guard requirements carried across an event edge, in two places:
db-effect guards (`pd_solve::cross_edge`, `pd_solve.rs:238`) and the cone's `substitute_entry`
(`capability_cone.rs:1602`). It can only do so for an ABI-derived parameter name that carries
quotes, because the lowerer already unquotes source names (probe f8). Expected movement on the
goldens and on CDO: none. The measurement plan checks it (step 5), and a unit row pins the
quoted-name case directly (a hand-built subscriber whose parameter name is `"Rec"`).

### 2. Dispatch bindings

**Edge metadata first.** The program engine merges one routine's subscriptions to one event
(per-build readings, `#if` arms, duplicate attributes) into one `Subscription` whose
`conditions` are the INTERSECTION over the readings (`index.rs:449-456`): a condition holds for
the edge only if every reading carries it. That is right for its existing consumers, but empty
does not mean "always runs": one reading may skip on a missing license while another does not.
So `Subscription` gains `conditional_any: bool`, the UNION (true when any reading has any
condition), computed in the same merge loop; `conditions` and its consumers are unchanged. The
model `EventEdge` gains `conditional: bool` = `conditional_any` (`events.rs` holds `s` when it
pushes the edge, probe f4). It is NOT added to `PEventEdge`, so the r2c goldens do not move. The
other `EventEdge` struct literals gain `conditional: false`: `src/engine/gate/events.rs:634`,
`src/engine/gate/format_html.rs:983`, `src/engine/l5/event_flow/tests.rs:36`,
`tests/cli/cli_c_events_differential.rs:668,676`. `CombinedEdge` gains
`dispatch: Option<DispatchMeta { conditional, isolated }>`, set only on `event-dispatch` edges,
from the edge and from `EventSymbol.isolated == Some(true)` (probe f6).

For a routine `P`, its dispatch bindings are built once per routine (like `walk_indexes`) from
the `event-dispatch` edges in `graph.edges_by_from[P]`. Two questions are kept apart: can we SEE
what the subscriber does (presence), and can we follow its effect on the record's state (path)?

- **Kind (visibility).**
  - `Exact { subscriber, q }`: the edge's `resolution == "resolved"` and the subscriber is in
    `routines_by_id`. Its role is visible.
  - `Opaque`: a non-`resolved` edge (the event-id collision of probe f2: the subscriber may bind
    a different overload) or a subscriber not in `routines_by_id`. Nothing about it is visible.
- **Flags.**
  - `optional = conditional || path_opaque || resolution != "resolved"`: it may not run.
  - `path_opaque = isolated || overload`: it runs, it is visible, but a failure rolls its
    database work back while its in-memory changes stay (probe f6), or it may belong to another
    overload, so its effect on the record's state is not one the role describes.
    `path_opaque` always implies `optional`.
  - **Overloads (step-6 Ruling, final-panel round 1).** Overloads of one event share one event
    id, so a subscriber that AL binds to ONE overload gets a dispatch edge from EVERY overload,
    and a smaller subscriber's parameter names all bind the larger overload too. The engine does
    not keep which overload a subscription selected. So `overload` is true on every edge whose
    event id more than one publisher routine raises (`shared_event_ids`, over the combined
    graph's `event-dispatch` edges): every binding of such a publisher is optional and
    path-opaque, never exact. A subscriber with a parameter (other than `Sender`) that binds no
    parameter of this overload is still `Opaque`.
- **Parameters.** For `Exact`, every subscriber parameter `q` with
  `event_publisher_param(P, S, q) = Some(p)` and `p` a record parameter of `P` gives a binding
  for `p`, with `caller_is_var = p.is_var`, `callee_is_var = q.is_var`. An `Opaque` binding
  applies to every record parameter of `P`, with `caller_is_var = p.is_var`,
  `callee_is_var = true` (we cannot see what the subscriber declares).
- **Dedupe** per subscriber `S`: if any edge to `S` gives an `Opaque` binding, it absorbs every
  `Exact` binding of `S` (one resolved and one ambiguous edge to `S` = `Opaque`). Otherwise dedupe
  by `(S, q)`: `optional` is the AND of the edges' flags (the subscriber is skipped only when
  every edge may skip it) and `path_opaque` the OR (isolated on any edge); code review F1-CR2.
  After the merge `path_opaque => optional` is enforced: a plain and an isolated edge to one
  subscriber give an isolated binding, which may not run to completion, so it never publishes a
  proven load (final-panel F1-CR2). Two edges to one subscriber are one subscriber.

`compose_roles_only` gains `routines_by_id` (already in `SccComputeCtx`). Routines without
dispatch edges pay one map lookup.

Where several bindings exist for one parameter `p`, call that number `n`.

### 3. Body-local presence facts (the c1b step)

Factor the body of the c1b loop (opaque branch, else branch, `mutates_param` recompute,
`summary_runner.rs:680-727`) into one function `fold_callee_presence(p, callee_role: Option<&RecordRoleSummary>)`
(`None` = opaque). The call-site loop calls it as today, unchanged. For each record parameter `p`
with dispatch bindings and `caller_is_var`:

- **Unseen dispatch** (any `Opaque` binding for `p`, or more than `EXACT_DISPATCH_MAX` = 4
  bindings): ONE call `fold_callee_presence(p, None)` and no per-subscriber fold. For an empty
  publisher body that publishes every presence fact of `p` as `unknown`: exactly the c1b opaque
  branch, the role an unseen callee gets. No `yes` from a visible subscriber survives next to it,
  so a forwarding caller cannot recover a load, a persist or a `validates yes` the path step did
  not follow (sol round 2).
- **Otherwise**, one call per binding with `callee_is_var`: the subscriber's role, or `None` for
  the three c1b opaque reasons (no summary, no role for `q`, no body). Isolated (`path_opaque`)
  bindings are folded like any visible one: their presence is real MAY information.

**MAY facts join; the MUST-read trio neither gains nor destroys a load.** The presence fields
split by how callers read them (probe c, verified per reader below):

- MAY facts (`persists_current_record`, `set_based_db_writes`, `validates_param`,
  `resets_filters_on_param`, and `mutates_param` derived from them): plain join, for every
  visible binding, optional or not. Order is free, and a subscriber that may not run adds `no`,
  the bottom.
- The `puts_in_loaded_state` trio (`loads_from_db_param`, `initialises_param`,
  `copies_into_param`): for an `optional` binding the subscriber's value goes through
  `optional_load(v)` = `yes -> no`, `unknown -> unknown`, `no -> no` before the join.

Why `no` and not `unknown` for an optional loader: such a subscriber either does not run (the
state is unchanged) or runs and loads (loadedness only grows). What each reader does with the
publisher's trio value:

| reader | trio `yes` | trio `unknown` | trio `no` |
|--------|-----------|----------------|-----------|
| caller walker (`cfg_walker.rs:1322-1334`) | `loaded = Yes` (gains a proven load) | `loaded = Unknown` (DESTROYS an earlier `Loaded::Yes`, sol round 2) | unchanged (identity) |
| c1b in the caller (join) | `yes` | `unknown` | caller's value unchanged |
| `puts_in_loaded_state` -> load points (`detectors/mod.rs:216-219`; d39 keeps a caller flagged, d40 drops a requirement) | load point | none | none |
| d40 / d42 (path facts, via the walker) | requirement dropped after the call | requirement kept, loaded fields `Unknown` | requirement as before the call |

`no` is the only value that neither invents a proven load nor destroys one: the caller sees the
run where the subscriber does not run, which is a real run, so a d40 finding there is real
(missing-load detectors stay sound), and d39 gets no load point. It loses only the MAY claim
"this may load", which no reader uses as MAY. `unknown` stays `unknown`: a subscriber whose own
load effect is unknown really may leave the record in any state. For an unconditional binding
the trio joins unchanged: `yes` from a subscriber that always runs is the claim a call site makes
today (the documented c1b leniency for a callee that loads on some path), not a new one.
**One rule for the mask (round 3).** `optional_load` is applied in exactly one place: the three
trio fields of the PUBLISHED role, because that is the only place they are read as a MUST load
(`puts_in_loaded_state`, and the walker's load-trio rules). Every MAY fact is computed from the
UNMASKED subscriber effects and travels to callers in a MAY field, never through a masked trio
field. Every place a trio value also feeds a MAY fact, checked against the source:

| trio value also feeds | MAY fact | how it stays right |
|-----------------------|----------|--------------------|
| `copies_into_param` in the `mutates_param` derivation (`summary_runner.rs:728-731`) | the publisher's `mutatesParam` | the publisher's `mutates_param` = `persists ⊔ validates ⊔ copies_unmasked` (an unmasked shadow kept during the dispatch fold). A sole manual `Copy` subscriber publishes `copiesIntoParam no`, `mutatesParam yes`. |
| the same derivation in a FORWARDING caller's c1b (it recomputes from its own, masked, `copies`) | the caller's `mutatesParam` | c1b ACCUMULATES `mutates_param` across call sites instead of recomputing it per call site: `p.mutates_param = join(p.mutates_param, derived, cr.mutates_param)` (the current line, `summary_runner.rs:728-731`, overwrites it with `derived` at every binding, so `OnManualCopy(R); NoOp(R)` would lose the first call's `yes`; round 4, sol). It is a join, so the result is independent of call order. For every existing callee `mutates_param` equals that same derivation over facts already joined into `p`, so adding `p.mutates_param` and `cr.mutates_param` to the join changes no call site today; it carries the unmasked mutation through forwarders. Rows 5d' (`OnManualCopy(R); NoOp(R)`) and 5d'' (reversed order) and a two-level forwarder pin it. Opaque branch unchanged (it already joins `unknown`). |
| `copies_into_param == Yes -> DirtyV` in the caller's walker (`cfg_walker.rs:1344-1348`) | the caller's dirt | the publisher's own path walk uses the UNMASKED subscriber roles (Design 4), so an optional copier puts `DirtyV` in the run branch and the join keeps it (`join_dirty`): the publisher's `dirtyAtExit yes`, then `cr.dirty_at_exit == Yes -> DirtyV` (`cfg_walker.rs:1365`) in the caller. Where the publisher has no path walk (`fixed_roles` dependency publishers, isolated and unseen dispatch: path facts `unknown`), the caller does not get that dirt: a stated leniency (fewer d39 findings), never an invented one. |
| `loads_from_db_param` / `initialises_param` | none | they feed only the load trio rules (MUST). |

No detector reads `mutates_param` (it is projected and part of the roles change key only); it is
fixed for correctness of the published summary.

This keeps the single unconditional subscriber exact (it equals a call site), and stops a
manual, conditional, isolated or collision subscriber from adding or removing a proven load for
any raiser.

### 4. Path-sensitive facts (the walker step)

Factor `cfg_walker::apply_call` into the binding search (unchanged) and
`apply_callee_role(state, callee_id, callee_param_index, caller_is_var, callee_is_var, …)`
(everything from the opaque guard down). `walk_param` gets the routine's dispatch bindings for this
parameter and applies the dispatch to every exit/fallthrough state before it aggregates
(`all_states`). AL requires a publisher's body to be empty (probe f5), so this is the dispatch on
the routine's initial state (`Pristine`, not loaded). Malformed source that has statements in a
publisher body (code AL rejects) is composed as body then dispatch; no runtime order exists to be
wrong about, and the census counts such publishers (Measurement step 6) so a non-zero count is
visible.

Write `f_i` for `apply_callee_role` with binding `i`, and `opaque` for the transfer of a callee
with no role (the existing opaque branch: entry requirements join `unknown` when not loaded; for
`var`/`var`, `loaded`, `dirty`, `pending_narrow`, loaded fields become `Unknown`). For one
parameter with `n` bindings:

- `n = 0`: unchanged.
- **Unseen dispatch (Design 3: any `Opaque` binding, or `n > EXACT_DISPATCH_MAX` = 4), or any
  `path_opaque` (isolated) binding:** `opaque(state)`, once. The publisher's own path facts for
  `p` become `unknown` (`dirty` `Unknown`, never `DirtyV`).
- **Otherwise, exact enumeration.** Let `U` be the always-running bindings and `O` the `optional`
  ones. The result is the `join_states` of `f_{π_k}(… f_{π_1}(state))` over every subset
  `S ⊆ O` and every order `π` of `U ∪ S`. Skipping every optional subscriber (`S = ∅`) leaves
  the sequences of `U` alone; that is the identity (the incoming state is one of the joined
  outcomes) ONLY when `U` is empty. With an unconditional `Get` and an optional no-op, every
  sequence runs the `Get`, so the incoming unloaded state is never an outcome (unit row 4b). With `n <= 4` that is at
  most 65 sequences of at most 4 transfers (all four optional: 1 + 4 + 12 + 24 + 24). `n = 1`
  unconditional is the single sequence `f_1(state)`, exactly a call site (the ControlDirect
  oracle). Each sequence is a concrete run, with no join inside it, so the result has the same
  precision as those calls written in source in that order, and it covers exactly the runs AL can
  make (every order of the subscribers that run; a conditional subscriber runs or does not). The
  transfers use the subscriber's own role, not `optional_load` (that mapping is for the published
  presence facts only, Design 3).

**What a caller sees after an UNSEEN dispatch** (an `Opaque` binding or `n > 4`; an isolated
binding alone is different, see "Isolated events" below: its MAY presence facts stay visible).
The published role of `p` is then every presence fact `unknown` (Design 3) and every path fact
`unknown`. A caller reads that like an unseen callee: c1b joins `unknown`; for a `var` publisher
parameter the walker sets `loaded`, `current_loaded_fields` AND `pending_narrow` to `Unknown`
(the `unknown` load-trio rule, `cfg_walker.rs:1330-1337`), so a `SetLoadFields` narrowing the
caller set before the call is NOT kept (unit row 12b); `dirty Pristine/Persisted -> Unknown`
(`cfg_walker.rs:1354-1360`); entry requirements join `unknown`; no load point, no d41
`resets yes`, no persist claim. One difference from the walker's true opaque branch
(`cfg_walker.rs:1255-1270`, used when the callee has no body or no role): a caller's OWN `DirtyV`
from before the call is kept rather than lowered to `Unknown`, because the `unknown` dirty rule
lowers only `Pristine`/`Persisted`. (For a by-value publisher parameter the load-trio rule needs
`callee_is_var`, so the caller's loaded state and narrowing are kept, as for any by-value
callee.) Neither invents anything about the subscriber; the kept `DirtyV` is the caller's own. So
an unseen dispatch never adds a finding the subscriber would be the cause of; it can keep one the
caller earned before the call.

Round-1 counterexample, `A = Validate; Modify` (role: validates yes, persists yes, dirty at exit
no) and `B = Modify`: `f_A(f_B(Pristine)) = f_A(Persisted) = Persisted` and
`f_B(f_A(Pristine)) = f_B(Persisted) = Persisted`, join `Persisted`, so `dirtyAtExit no`. The
round-1 closure joined `X_0 = Pristine` with `Persisted` into `Unknown` and then `f_A(Unknown)`
fired the `validates && Unknown -> DirtyV` rule. Enumeration never feeds a joined state back into
a transfer, and any `Opaque` or `path_opaque` binding collapses the whole dispatch to one
`opaque` (after which no transfer runs). A transfer inside a dispatch can still see `Unknown`, but only when an earlier
subscriber's OWN role is `unknown` (for example it calls something opaque), which is exactly what
the same two calls in source would give. That `f(Unknown)` rule is order-blind for a callee that
validates and then persists; it is pre-existing call-site behaviour (`Opaque(R); A(R)` in source
hits it today), so it is filed as a discovery, not changed here.

No closure, `LOOP_BOUND` or `saturate_unknown` is used, so nothing lowers `DirtyV` to `Unknown`.

**Isolated events.** A failing isolated subscriber is rolled back in the database while its
in-memory changes to `var` parameters stay, and the publisher continues (probe f6). Presence and
path are separated (Design 2): the subscriber is `Exact` (visible), `optional`, and
`path_opaque`. Its presence facts join as MAY facts (`persistsCurrentRecord yes`,
`validatesParam yes` are true on the run where it succeeds), its load trio goes through
`optional_load`, and the publisher's path facts for `p` take the opaque path above, because
after a failure the record's state is not any prefix the role describes (a `Modify` may be
undone while the fields stay changed). A forwarding caller reads the visible MAY facts through
the call-site rules: `validatesParam yes` without `persistsCurrentRecord yes` takes it to
`DirtyV`, right in the MAY sense d39 uses ("dirty on some path": validated, then failed, fields
changed and unsaved). With `persistsCurrentRecord yes` too, the persist rule runs first and the
caller stays `Persisted` (`cfg_walker.rs:1338-1346`), which misses the failed run's dirt: a
leniency, never an invented finding.

### 5. Solver order

**Ordering: no change needed.** Probe (e): subscribers settle before a non-recursive publisher
and are read from `final_map`; a publisher-subscriber cycle is a recursive SCC whose Jacobi
dependents already include dispatch edges (`summary_runner.rs:1704-1713`), so a subscriber's role
change re-dirties the publisher. The SCCs themselves do not change: dispatch edges are already in
every SCC input graph; only what the roles step reads along them changes.

**Convergence: no monotonicity claim.** The roles transfer is NOT monotone, and was not before
this change: a callee's `loadsFromDbParam` moving `no -> yes` makes the caller's walker
`loaded = Yes`, which can move a later requirement `yes -> no` (`cfg_walker.rs:1297-1337`).
Jacobi over a finite lattice with a non-monotone transfer can oscillate, and only the cap
(`MAX_FIXED_POINT_ITERATIONS`) stops it. Two things make that honest:

1. **A guard.** Today a capped SCC keeps its round-MAX roles (`summary_runner.rs:1794-1830`),
   which may be any point of an oscillation. This change replaces every capped member's roles
   with the opaque role (every presence fact `unknown`, every path fact `unknown`, field lists
   `Unknown`), the role a reader already treats as "cannot see". The `fixpoint-capped`
   uncertainty and diagnostic stay. A `fixed_roles` member (Design 6) keeps its base roles, but
   its dispatch presence was folded from the unconverged snapshot, so on a cap it is refolded as
   an unseen dispatch (presence `unknown` for every dispatched parameter; final-panel F1-CR3).
   The guard fires only on a cap, which no corpus SCC reaches
   today (`RolesSccOut` doc), so it moves nothing unless a cap happens.
2. **A measurement.** Roles cap hits on the goldens and on CDO, before and after: must stay 0
   (Measurement step 4). Unit row 10 runs `ws-cycle` (a recursive loader/persister cycle across a
   dispatch edge) and asserts convergence without a cap.

### 6. Dependency publishers

- Source-bearing (cross-app, `fixed_roles`): `member_roles` keeps the base roles and then runs step
  3 only (the presence fold over dispatch bindings, with `optional_load` for optional ones and the unseen-dispatch rule). Step 4
  does not run, so the five path-sensitive fields stay `unknown`, as #70 states. Workspace and
  dependency subscribers are treated alike (both are dispatch edges of the publisher). #70's
  `Fwd` expectation (entry fields `unknown`, `dirtyAtExit no`, loaded `full`) still holds:
  `persists yes` takes the walker from `Pristine` to `Persisted`, which still counts as `no`.
- **Symbol-only (bodyless): both steps are skipped; its roles stay the base roles.** Decision and
  why: every bodyless callee in the engine carries its base roles, and the guarded readers (c1b,
  walker, d37, d39) ignore them. Composing subscribers into a bodyless PUBLISHER would make it the
  one bodyless callee whose `resets_filters_on_param` and load trio the two unguarded readers
  (`d41`, `record_load_points`, probe b2) act on, a new, publisher-only behaviour of a guard gap
  that concerns every bodyless callee. The value is invisible to every guarded reader, so skipping
  loses nothing. The gap itself is filed as a discovery (fix it once, for all bodyless callees).
  Unit row 8 pins "identical to base".
- Single-app: no dispatch edge, no change (probe b3).

### 7. Every output that can move

- `RoutineSummary.parameter_roles` of every BODIED publisher with a subscriber that touches a
  record parameter by name, and transitively of every routine that calls such a publisher with a
  record argument: through c1b and the walker's exit effects for `var` arguments, and through the
  walker's entry requirements (c1a) for by-value arguments too (`cfg_walker.rs:1297`).
- `tests/r3a2-goldens/*` (`parameterRoles`), `tests/r3a5-goldens/*`, the r3a2/r3a5 manifests.
- Detector findings, default set: `d37-validate-without-persist` (a caller's chain now persists:
  fewer findings), `d39-record-left-dirty-across-chain` (publishers with a dirtying subscriber
  become dirty callees: new findings; a dispatch load point after a call keeps a caller flagged),
  `d41-transitive-filter-loss` (a subscriber `Reset`s: new findings), `d42-cross-call-wrong-setloadfields`
  (field lists). Opt-in: `d40-transitive-load-missing` (raising an event whose subscriber
  modifies an unloaded record: new findings; a dispatch load drops a later requirement). Through
  them: `tests/r4-goldens`, `tests/cli-*-goldens`, `tests/gate-goldens` if a fixture raises an
  event with a record argument.
- Guard requirements carried across event edges, from the quote-trim unification (Design 1):
  db-effect guards (`cross_edge`) and the cone (`substitute_entry`). Expected unchanged (source
  names are unquoted); measured.
- A capped roles SCC's roles (Design 5 guard); none today.
- Not moved: `d3-missing-setloadfields` (escapes every publisher callee, reads only `may_*`), call
  edges, the histogram (`--program-call-graph-stats`), db-effect rows, the event graph and its
  projection (`conditional` is not projected), symbol-only publishers and their callers.

## False-positive and failure shapes

- **MAY loads read as MUST (rounds 1 and 2).** Callers read the load trio as "loaded after the
  call" (drops d40/d42 requirements, adds d39 load points), and read `unknown` as "loadedness
  lost" (destroys an earlier `Loaded::Yes`, `cfg_walker.rs:1330-1334`). Design 3's
  `optional_load` publishes an optional loader as `no`, the identity in every reader (Design 3
  table), so a subscriber that may not run neither adds nor removes a proven load. An
  unconditional subscriber that loads on SOME path still gives `yes`, the same leniency every
  call site has today; on CDO, every REMOVED d40/d42 finding and every NEW d39 finding whose only
  new load point is a dispatch is tagged in triage.
- **Mixed build readings.** A subscriber conditional in one `#if` reading and not in another is
  `optional` (`conditional_any`, Design 2), never "always runs".
- **New d40 findings** on a caller that raises `OnX(var Rec)` with an unloaded `Rec` while a
  subscriber modifies it. Real code often raises `OnBefore…` events with a record that IS loaded;
  then the walker's `loaded = Yes` blocks the requirement. Triage every new d40 finding on CDO.
- **New d39 findings** where a subscriber `Validate`s and the raiser persists afterwards: d39's
  existing `skipped_caller_persists` path handles it; triage confirms.
- **Order-blind role on an `unknown` input.** Inside an exact enumeration a subscriber with an
  `unknown` role can hand the next one an `Unknown` dirty state, and `validates && Unknown ->
  DirtyV` then fires even if that next subscriber persists last. Same as the same calls in source
  (Design 4); filed as a discovery.
- **`unknown` widening**: more than 4 bindings or an `Opaque` binding publish every presence and
  path fact of `p` as `unknown` (Design 3, Design 4 "What a caller sees on the opaque path"); an
  isolated event makes the path facts `unknown` and keeps its visible MAY presence facts. Neither
  adds a finding caused by the subscriber; a caller's own earlier `DirtyV` is kept. Measured on
  CDO as a finding drop, with the number of publishers that took this path (census).
- **Opaque subscribers** (non-`resolved` edge, subscriber not in the model) and visible
  subscribers with no body, summary or role: presence `unknown` through the c1b opaque branch;
  for the first kind the whole dispatch of `p` is unseen (no visible subscriber's `yes` survives
  next to it).
- **Isolated subscriber that validates.** Its `validatesParam yes` reaches a forwarding caller,
  which goes `DirtyV` unless the subscriber also persists. That is a real MAY state (validated,
  then failed and rolled back, fields still changed); triage new d39 findings that come only from
  an isolated event.
- **Name collisions**: a subscriber parameter named like a non-record publisher parameter is
  ignored (only record parameters of `P` get roles). `Sender` on a codeunit event matches no
  publisher parameter; on a table event it is the raising record and is not modelled (Non-goals).
- **Event-id collisions**: an ambiguous subscription that shares a real publisher's id is an
  `Opaque`, `optional` binding (Design 2), never an exact one.
- **Convergence**: a cycle that does not settle hits the cap; its members get the opaque role
  (Design 5 guard), so a capped oscillation cannot fire a detector.

## Measurement plan

On the pinned CDO workspace `U:/Git/DO-cdo-baseline/Cloud` (CDO_WS), `cdo-measure` skill, release-fast
`alsem` built from base `8405f96d` and from the branch head:

1. Two runs per binary, each at `--scope primary` and `--scope all`, cross-app (the default):
   - `alsem analyze <ws>` with the DEFAULT detector set (it includes `d3-missing-setloadfields`,
     `d37-validate-without-persist`, `d39-record-left-dirty-across-chain`,
     `d41-transitive-filter-loss`, `d42-cross-call-wrong-setloadfields`, and every non-role
     detector);
   - `alsem analyze <ws> --detector d40-transitive-load-missing` (opt-in, so it never runs in the
     default set).
   Findings per detector (full ids) before/after and fingerprint churn. Expected to move:
   d37, d39, d41, d42, d40. Every other detector, `d3` included, byte-identical.
2. Population: from `--r3a5-cross-app-summary` before/after, the number of publishers whose role
   moved, of callers whose role moved, split by `var` vs by-value argument.
3. Triage every NEW d39/d40/d41/d42 finding against source (`triage-findings`), and give every
   REMOVED finding a one-line reason (a subscriber persists/loads/resets), tagging those whose
   only cause is a dispatch load point (FP section). If new findings of one detector are >30%
   false positives, fix the cause, not the detector's default.
4. Roles cap hits (`fixpoint-capped`, summarize diagnostics) before/after: 0 and 0.
5. Byte-identity checks for what should not move: `aldump --program-call-graph-stats`
   histogram; the db-effect store and cone projections (`--r3a3-cone-coverage`,
   `--r3a5-cross-app-summary` db-effect/cone fields), which the quote-trim unification could
   touch (Design 1); the `tests/r2c-goldens` event-graph family (via `scripts/check-goldens`).
6. Performance attribution from a census (in `summaries_census`, off unless the census is on):
   dispatching publishers, bindings per (publisher, parameter) histogram and max, how many took
   the exact path vs the opaque path (`n > 4`, `Opaque`, isolated), sequences enumerated in total
   and max per walk, roles Jacobi rounds in SCCs that hold a dispatch edge, publishers with a
   non-empty body (expected 0), subscriptions whose `conditional_any` differs from
   `!conditions.is_empty()` (mixed readings), plus `alsem analyze` wall time and peak memory
   before/after.

## Discoveries to file

- `d41` and `record_load_points` read a bodyless callee's role without a `body_available` guard
  (probe b2); every bodyless callee, not only publishers.
- The call-site transfer's `validates && dirty Unknown -> DirtyV` rule ignores a callee that
  validates and then persists (Design 4); reachable from source call sequences today.
