# Record operations raise their table's platform events (F1): design note

**Status:** implemented with this note. **Date:** 2026-11-06. **Based on:** master `6e577c24`.
**Closes:** gap F1 of `2026-10-13-callgraph-export-design.md` (§3 fact 9, §9) and the table half
of row A12 in `docs/al-compiler-callgraph-holes.md`. **Found by:** call c11 of the 2026-11-06
output validation (`target/tmp/output-validation/round1/factcheck/c11-c20.md`).

## 1. The defect, and whether it was deliberate

Business Central raises a table's **database trigger events** on every record operation:

| Operation | Events raised (before, after) |
|-----------|-------------------------------|
| `Insert` | `OnBeforeInsertEvent`, `OnAfterInsertEvent` |
| `Modify`, `ModifyAll` | `OnBeforeModifyEvent`, `OnAfterModifyEvent` |
| `Delete`, `DeleteAll` | `OnBeforeDeleteEvent`, `OnAfterDeleteEvent` |
| `Rename` | `OnBeforeRenameEvent`, `OnAfterRenameEvent` |
| `Validate(F, …)` | `OnBeforeValidateEvent`, `OnAfterValidateEvent` for field `F` |

Their subscribers run in the caller's transaction, so their writes, locks, errors and commits
are the operation's. The engine never followed them: the resolver gave a record operation an
`ImplicitTrigger` edge to the table's own triggers only, and the site rule then dropped even
those for `RunTrigger = false`. So `alsem prove OnAfterCopyPurchaseDocument
writes-table:"Purch. Inv. Entity Aggregate"` answered `no` with complete coverage, though its
`Modify(false)` raises Purchase Header `OnAfterModifyEvent` and the Base Application
subscriber `PurchInvAggregator.Codeunit.al:58` locks and writes that table.

**It was a known gap, not a choice for cost.** The 2026-10-13 call-graph export spec lists it
as fact 9 and gap **F1** ("Record operations do not raise platform table events … closes when
the resolver adds routes from each record operation on T to T's synthetic publishers for that
op; then rule (a) of `unraisedPublisher` is deleted"), flagged every synthetic table publisher
`unraisedPublisher`, and `test_select` printed table events as never covered. No spec or
commit gives cost or hub size as a reason; the synthetic publishers
(`build::inject_platform_event_publishers`, 2026) were built so subscribers bind, and the raise
side was left for later. Meanwhile `prove`'s `relies_on_loaded_subscribers` doc claimed the
cone already follows them, so its `loaded-subscribers` label promised coverage that was not
there.

## 2. The platform facts this relies on

- **RunTrigger gates only the table's own trigger.** The events fire for `Modify(false)` too:
  the subscriber receives the flag as its `RunTrigger` parameter (Microsoft Learn, "Event
  types", database trigger events; the order is before-event, table trigger, global trigger,
  database operation, after-event).
- **`ModifyAll` and `DeleteAll` raise them once per row.** Microsoft Learn ("AL database methods
  and performance on SQL Server", ModifyAll and DeleteAll) says both fall back to row-by-row
  calls when the table has subscribers to the modify or delete events. The row set may be
  empty, so the edge is `Maybe`. `ModifyAll` never validates, so it raises no validate event.
- **Temporary records raise them too.** Microsoft's own subscribers guard with
  `if Rec.IsTemporary then exit` (27 such `OnAfterModifyEvent` subscribers in the Base
  Application of the CDO baseline's `.alpackages`), and `alguidelines.dev` ("IsTemporary record
  safeguard") gives the same advice. The task brief assumed the opposite; the evidence says
  they are raised, so a temporary record keeps its edge. What the subscriber then writes is
  judged as usual: a write to the event's own `Rec` takes the operation's temp state (§3.3), a
  write to another table is physical.
- **Global triggers are not table events.** `OnDatabaseInsert`/`OnGlobalModify`
  (codeunit 2000000002 "Global Triggers", re-published by Base Application codeunit 49) run for
  every table a subscriber enables at run time through `GetDatabaseTableTriggerSetup`. Not
  modelled here: see §6.

## 3. The model

### 3.1 Program engine (`src/program/resolve`)

A record operation on a resolved table gets a **second edge** at its site, beside its
`ImplicitTrigger` edge: `EdgeKind::TableEvent`, under its own obligation
`ObligationId::TableEvent { caller, span, callee_fp }` (the site's own key, as
`ObligationId::Background` does for a background start). Its routes are the table's synthetic
`PublisherKind::Platform` publishers of the events the operation raises
(`resolver::resolve_table_events`, `resolver::table_event_names`); each publisher's existing
`EventFlow` edge carries its subscribers. So the record operation reaches the subscribers the
way a call reaches an integration event's publisher.

- The site rule (`TriggerSiteRule`) is not applied: RunTrigger does not gate events.
- A publisher exists only when a loaded app subscribes to the event, so an operation on a table
  nothing subscribes to gets **no** `TableEvent` edge (no new honest-empty obligations).
- Shape `Multicast`, completeness `Partial { ReverseDependentSubscribers }`: an app not loaded
  may subscribe too, as for any event. Each route has the table's tier evidence (`Source` or
  `Abi`) and the new `Witness::Platform` (the publisher has no declaration to cite; the
  route-applicability contract accepts it with `Source`/`Abi` evidence and a `Routine`
  target only).
- The lookup goes through the resolver's reads facade (`routines_named`), so the workspace
  session's S rule re-resolves a dependency file when a publisher appears or goes.
- **Validate** reaches the publisher of the event, which collapses the fields (one synthetic
  publisher per table and event), so at this level `Validate(A)` reaches the subscribers of
  every field. The detector model narrows it (§3.2). The program-level consumers (the
  call-graph stats, `alsem graph`, demand) take that over-approximation.

### 3.2 Detector model (`program::model::program_calls`)

For each record operation matched to its `ImplicitTrigger` edge, the adapter finds its
`TableEvent` edge by obligation, reads each publisher's `EventFlow` routes, and emits one
model `CallEdge` per model subscriber: `DispatchKind::TableEvent` (`"table-event"`), keyed by
the operation like an implicit trigger (`callsite_id = operation_id = op id`), with
`table_event: { event_id, conditional }`. The event id is the event graph's (with the field for
a validate event), so the edge and the event graph name one event. A `Validate` keeps only the
subscribers whose `[EventSubscriber]` element is its own field. `fires` is `Always`, `Maybe`
for `ModifyAll`/`DeleteAll`; the resolution is `Maybe` for those and for a subscription that
may be skipped. A subscriber outside the model (a dependency subscriber in a single-app model)
gets no edge; its identity goes to `external_targets`, as a dependency trigger's does (S3.6).

### 3.3 What follows the new edge kind

| Consumer | Treatment |
|----------|-----------|
| combined graph / typed graph (`combined_graph.rs`) | edge kind `table-event`, `event_id` and dispatch metadata set; typed edge keyed by `operation_id` |
| capability cone, prove, digest, explain, query effects/touches | followed like any edge; inherited facts get `via: event-dispatch` (no new via value) |
| temp-state binding (`summary_runner::table_event_param_temp_state`, `pd_solve::cross_edge`) | each RECORD parameter of the subscriber (`Rec`, `xRec`) binds to the raising operation's temp state, as a trigger's `Rec` does; anything else is `Unknown`; guards are dropped (sound) |
| snapshot graph (`SnapshotGraphEdge::TableEvent`, kind `table-event`) | kept like an implicit trigger, never dropped for a missing anchor; its `PdEdgeKey` carries the event, so one op reaching one subscriber through two events stays two edges |
| digest witness paths | shown as an event-dispatch hop to the subscriber; a fact's first hop carries both the op and the event (`FirstHop::OpEvent`), so the op's trigger edges never pass for it |
| literal reach | leaves from the raising operation, whose guards apply |
| ordering (`ordering_inter`) | keyed by the operation, an unordered event broadcast |
| explain callers and rules (`cross_rules`) | skipped: a subscriber is dispatched, not called, as an integration event's subscriber is (the model's calls hold no publisher → subscriber edge), so the raiser is no caller of it and inherits none of its rules |
| walkers that leave event fan-out to another detector (d1, d1 graph, d2's evidence walk, d46, d48, d54) | skip `table-event` as they skip `event-dispatch` (`combined_graph::is_subscriber_edge`) |
| test coverage | counts as crossing an event |
| impact keys (`impact/keys.rs`), call-site resolution ledger, closed-interface sites | not a call site: skipped like an implicit trigger |
| demand (`resolve/demand.rs`) | followed forward; NOT a "raiser" for the reverse rule (a platform event has no IsHandled for d43, and every writer of `Customer` would be demanded) |
| `alsem graph` / sited graph | `Event` edge, `via: "tableEvent"` (additive `via` value, no schema bump), boundary `TriggerOf` the raising operation; a code-call site (`reverse::is_code_call_site`). `unraisedPublisher` on a synthetic table publisher now means no non-hub edge raises it (rule (a) narrowed; a record hub edge still does not clear it) |
| test selection | the `platform-table-event` hidden channel is printed only for such an unraised publisher |
| LSP | outgoing calls of a routine list the publisher; `LspEdge::obligation_id` knows the kind |
| graphify export | `raises_event` |
| compiler oracle | left out: altool never raises a platform event (A12), so there is nothing to compare |
| d7 (event recursion), d2 (event in a loop), d43/d44/d45 | unchanged rules. d43–d45 read the event graph, which already held platform events and their subscribers; their findings move only through the cone facts they read |

## 4. Honesty labels

- `prove`'s `loaded-subscribers` assumption now describes what the cone does for table events
  too: it follows every loaded subscriber, and a `no` stays labelled because an unloaded app
  may subscribe.
- Table events a `RecordRef` operation raises are not followed per table (the table is not
  known); the open record hub keeps such a site open (`OpenBound::Record`), as before.
- Global triggers (§6) are not followed; this note does not change any label for them.

## 5. Cost on CDO

Measured with release-fast builds, before (`6e577c24`) and after, on the pinned baseline
(`/home/sshadows/Projects/DO-cdo-baseline/Cloud`, `bc3ccb18`):

- **Edges.** 98 new `TableEvent` edges at workspace record ops, 6,132 in dependency bodies
  (5,859 in the Base Application), all resolved; `unknown` stays 0 in both. Only record ops
  on a table some loaded app subscribes to get one, so no hub forms: each edge has at most
  two routes (the before and after publisher).
- **The cone grows where it should.** The cross-app model demands the subscribers of the
  table events the workspace raises and their cones; on CDO the `--scope all` findings grow
  by 1,189 (almost all in that newly demanded dependency code), the `--scope primary` ones by
  15. A table event with many subscribers (Customer, Job Queue Entry, Sales Line) pulls in all
  of them; that is the platform's behaviour, not an over-approximation.
- **Time and memory** of one explain and one prove: the CHANGELOG entry gives them (the host
  ran at a load of 150-400 on 22 cores during the measurement, so wall times are noisy; user
  CPU time is reported beside them).

## 6. Not done here

- **Global triggers** (`OnDatabaseInsert/Modify/Delete/Rename`, `OnGlobal*`): table-agnostic,
  enabled per table at run time, subscribed by Change Log, API webhooks and CRM integration.
  Following them from every record op would put most of the Base Application's change-tracking
  code in every cone. Gap F8 ("platform entry points … global triggers") of the export spec
  stays open.
- **Table-extension triggers** `OnBefore/OnAfter<Op>` and field `OnBefore/OnAfterValidate`
  (gap F5) are a different channel (triggers, not events) and stay flagged
  `unmodeledRecordTrigger`.
- **d2 for table events** (a record op in a loop whose table-event subscribers touch the
  database) and **d7 for table-event recursion** (a subscriber that modifies its own table):
  new detector rules, to be measured and triaged on their own.
- **Single-app models** keep no edge to a dependency subscriber (the S3.6 precedent for
  dependency triggers); `alsem prove`, `explain` and `query` are cross-app by default.
