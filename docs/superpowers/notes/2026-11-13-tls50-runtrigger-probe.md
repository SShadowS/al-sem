# TLS-50: what `RunTrigger = false` runs (BC probe)

**Status: measured on BC 28 Linux, Windows pending.** Per CLAUDE.md, a sidecar fact becomes an
engine fact only after a Windows Cronus confirms it.

## Question

1. Does a record operation with `RunTrigger = false` run any table trigger (`On<Op>`) or
   table-extension trigger (`OnBefore<Op>`, `OnAfter<Op>`)? The operations are `Insert(false)`,
   `Modify(false)`, `Delete(false)`, `ModifyAll(…, false)` and `DeleteAll(false)`. The same
   question applies with the argument left out.
2. Do the table events (`OnBefore/OnAfter<Op>Event`) run?
3. For ENG-71: what do `Rec` and `xRec` hold in a table extension's `OnBefore<Op>`/`OnAfter<Op>`?
   Does a change made to `Rec` there reach the write?

The engine's rule is `applicability::TriggerSiteRule`: a literal `false` or a missing argument
admits no trigger route. It is applied in `full.rs` and in `impact_graph/project.rs`. Table
events are resolved without that filter (`resolver::table_event_names`'s doc: "BC raises these
whatever the RunTrigger argument says"). ENG-15 relies on the same rule for its symbol-only gap.

## Environment

- BC 28.4 on the kraken Linux sidecar (`http://bc`, instance `BC`, tenant `default`).
  Platform (System) 28.0.54946.56477; Application 28.4.53241.56590.
- Compiled with alc 18.0.41.62505 (`/opt/al-tool/al compile`); published at Dev scope with
  `bcdev_app_publish`; run with `bcdev_test_run`; both apps unpublished afterwards.
- Probe source, in `docs/superpowers/notes/tls50-probe/`:
  - app 1: `app.json`, `src/Probe.al`;
  - app 2, the cross-app extension: `ext/app.json`, `ext/src/Ext.al`.

## The probe

App 1 holds:
- table `TLS50 Item` (`PK`, `Val`) with `OnInsert`, `OnModify` and `OnDelete`;
- a `tableextension` of it with `OnBefore`/`OnAfter` `Insert`, `Modify` and `Delete` (logged
  `X.`);
- subscribers to the six table events (`E.`).

App 2 depends on app 1 and holds a second `tableextension` of `TLS50 Item` with the same six
triggers (`Y.`). It is the cross-app shape that ENG-15's symbol-only gap is about.

Every trigger and subscriber appends one line to a log table that has no triggers or
subscribers, written with `Insert(false)`. Each line records:
- the source: `T.` is the table, `X.`/`Y.` an extension, `E.` an event;
- `rec`: `Rec.Val`;
- `x`: `xRec.Val`, for the triggers and the Modify events;
- `rt`: the event's `RunTrigger`;
- `db`: the stored row's `Val`, read with a `Get` on another record variable; `-1` when no row is
  stored.

There is one `[Test]` method per operation and argument (`false`, `true`, left out). Each starts
from a row seeded with `Val = 1`; Insert starts from none. It sets `Val` and calls the
operation. `ModifyChangedInOnBefore` is the one exception: there, app 1's `OnBeforeModify` adds
100 to `Val` for that row. Each test reads the log before raising it as its error, in the same
transaction. That is how the result leaves the test, so every test reads "failed" by design.

## Raw results (2026-11-13, second run: both apps published)

| Test | Log, in order |
|---|---|
| InsertFalse | E.OnBeforeInsertEvent rt=No rec=5 db=-1 \| E.OnAfterInsertEvent rt=No rec=5 db=5 |
| InsertOmit | E.OnBeforeInsertEvent rt=No rec=5 db=-1 \| E.OnAfterInsertEvent rt=No rec=5 db=5 |
| InsertTrue | E.OnBeforeInsertEvent rt=Yes rec=5 db=-1 \| X.OnBeforeInsert rec=5 x=5 db=-1 \| Y.OnBeforeInsert rec=5 x=5 db=-1 \| T.OnInsert rec=5 x=5 db=-1 \| X.OnAfterInsert rec=5 x=5 db=5 \| Y.OnAfterInsert rec=5 x=5 db=5 \| E.OnAfterInsertEvent rt=Yes rec=5 db=5 |
| ModifyFalse | E.OnBeforeModifyEvent rt=No rec=2 x=2 db=1 \| E.OnAfterModifyEvent rt=No rec=2 x=2 db=2 |
| ModifyOmit | E.OnBeforeModifyEvent rt=No rec=2 x=2 db=1 \| E.OnAfterModifyEvent rt=No rec=2 x=2 db=2 |
| ModifyTrue | E.OnBeforeModifyEvent rt=Yes rec=2 x=2 db=1 \| X.OnBeforeModify rec=2 x=2 db=1 \| Y.OnBeforeModify rec=2 x=2 db=1 \| T.OnModify rec=2 x=2 db=1 \| X.OnAfterModify rec=2 x=2 db=2 \| Y.OnAfterModify rec=2 x=2 db=2 \| E.OnAfterModifyEvent rt=Yes rec=2 x=2 db=2 |
| ModifyChangedInOnBefore | E.OnBeforeModifyEvent rt=Yes rec=2 x=2 db=1 \| X.OnBeforeModify rec=2 x=2 db=1 \| Y.OnBeforeModify rec=102 x=2 db=1 \| T.OnModify rec=102 x=2 db=1 \| X.OnAfterModify rec=102 x=2 db=102 \| Y.OnAfterModify rec=102 x=2 db=102 \| E.OnAfterModifyEvent rt=Yes rec=102 x=2 db=102 \|\| caller Rec after=102 stored after=102 |
| DeleteFalse | E.OnBeforeDeleteEvent rt=No rec=9 db=1 \| E.OnAfterDeleteEvent rt=No rec=9 db=-1 |
| DeleteOmit | E.OnBeforeDeleteEvent rt=No rec=1 db=1 \| E.OnAfterDeleteEvent rt=No rec=1 db=-1 |
| DeleteTrue | E.OnBeforeDeleteEvent rt=Yes rec=9 db=1 \| X.OnBeforeDelete rec=9 x=9 db=1 \| Y.OnBeforeDelete rec=9 x=9 db=1 \| T.OnDelete rec=9 x=9 db=1 \| X.OnAfterDelete rec=9 x=9 db=-1 \| Y.OnAfterDelete rec=9 x=9 db=-1 \| E.OnAfterDeleteEvent rt=Yes rec=9 db=-1 |
| ModifyAllFalse | E.OnBeforeModifyEvent rt=No rec=3 x=3 db=1 \| E.OnAfterModifyEvent rt=No rec=3 x=3 db=3 |
| ModifyAllOmit | E.OnBeforeModifyEvent rt=No rec=3 x=3 db=1 \| E.OnAfterModifyEvent rt=No rec=3 x=3 db=3 |
| ModifyAllTrue | E.OnBeforeModifyEvent rt=Yes rec=3 x=3 db=1 \| X.OnBeforeModify rec=3 x=3 db=1 \| Y.OnBeforeModify rec=3 x=3 db=1 \| T.OnModify rec=3 x=3 db=1 \| X.OnAfterModify rec=3 x=3 db=3 \| Y.OnAfterModify rec=3 x=3 db=3 \| E.OnAfterModifyEvent rt=Yes rec=3 x=3 db=3 |
| DeleteAllFalse | E.OnBeforeDeleteEvent rt=No rec=1 db=1 \| E.OnAfterDeleteEvent rt=No rec=1 db=-1 |
| DeleteAllOmit | E.OnBeforeDeleteEvent rt=No rec=1 db=1 \| E.OnAfterDeleteEvent rt=No rec=1 db=-1 |
| DeleteAllTrue | E.OnBeforeDeleteEvent rt=Yes rec=1 db=1 \| X.OnBeforeDelete rec=1 x=1 db=1 \| Y.OnBeforeDelete rec=1 x=1 db=1 \| T.OnDelete rec=1 x=1 db=1 \| X.OnAfterDelete rec=1 x=1 db=-1 \| Y.OnAfterDelete rec=1 x=1 db=-1 \| E.OnAfterDeleteEvent rt=Yes rec=1 db=-1 |

A first run (app 1 only; no `Omit` tests and no change test; no `xRec` logged in the Modify
events) gave the same lines without the `Y.` entries.

## Findings

Scope: the shape run here. That is one row, code-driven calls, a subscriber to every table event
present, and the table and one extension in one app with a second extension in another app.

1. **`RunTrigger = false`, and the argument left out, run no table trigger and no
   table-extension trigger**, `OnBefore` and `OnAfter` included. This holds for an extension in
   the table's own app and for one in another app. It holds for `Insert`, `Modify`, `Delete`,
   `ModifyAll` and `DeleteAll`.
2. **The table events run in every case**, with `RunTrigger` passed through (`No` for `false`
   and for the argument left out, `Yes` for `true`). With a subscriber present, `ModifyAll` and
   `DeleteAll` raised the events for their one row, so only the row-by-row path ran.
3. **With `true` every extension trigger runs, in either app.** The order:
   1. `OnBefore<Op>Event`;
   2. the extensions' `OnBefore<Op>` (in this run app 1's, then app 2's; what orders two
      extensions was not measured);
   3. the table's `On<Op>`;
   4. the write;
   5. the extensions' `OnAfter<Op>`;
   6. `OnAfter<Op>Event`.

   "The write" is as a read sees it: a `Get` in the same session sees the stored row change
   between the table's `On<Op>` and the extensions' `OnAfter<Op>`.
4. **What `Rec` and `xRec` hold (for ENG-71).**
   - In the extensions' `OnBefore<Op>` and the table's `On<Op>`:
     - `Rec` holds the caller's values;
     - a read still sees the old row (`Modify`, `Delete`) or none (`Insert`).
   - In `OnAfter<Op>`, `Rec` holds the same values and a read sees the write.
   - **A change to `Rec` in `OnBeforeModify` carries forward** (Modify only; Insert and Delete
     not measured). App 1's `OnBeforeModify` set
     `Val` to 102. App 2's `OnBeforeModify`, the table's `OnModify`, the write, the `OnAfter`
     triggers, the `OnAfterModifyEvent` and the caller's own record variable all saw 102.
   - **`xRec` is the caller's values, not the stored row**, in every trigger and in the Modify
     events, in these code-driven calls. It also kept the caller's value (2) after an
     `OnBefore<Op>` changed `Rec` (102). Whether a page call differs was not measured.

## The engine against this

- **`false` and a missing argument agree.** `TriggerSiteRule` gives a literal `false` or a
  missing argument (`Some(false)`) no trigger route, at every fan-out site. Table events are
  resolved without that filter, so they stay. A subscriber that starts with
  `if not RunTrigger then exit` is dropped for `false` (`RaiseContext::kills`), and `rt=No` above
  supports that. Earlier, on Windows Cronus28 on 2026-10-07, the missing argument had been
  measured only for the table trigger (`applicability.rs`, the `TriggerSiteRule` doc). This run
  adds both extension triggers, for the sidecar.
- **`true` shows that ENG-15's fix is needed on master.** Master's `resolve_implicit_trigger`
  (`resolver.rs`) looks up only `on<op>` in each extension, so an extension's `OnBefore<Op>` and
  `OnAfter<Op>` get no edge, while BC runs them in both apps. That is engine-limits B11, fixed
  only on the unmerged `alsem/eng-15-ext-triggers`.
- **No code changes in TLS-50.**

## B11 (on `alsem/eng-15-ext-triggers`, not on master)

The ENG-15 lane updates B11 before it lands (orchestrator, 2026-11-13). The sentence "`RunTrigger`
false runs no extension trigger either (not re-measured on BC)" should become: "`RunTrigger`
false, or the argument left out, runs no table or extension trigger, in the table's app or
another; the table events still run with `RunTrigger` passed through (measured on BC 28 Linux,
Windows pending, scope in the note: TLS-50,
`docs/superpowers/notes/2026-11-13-tls50-runtrigger-probe.md`)."

Every B11 claim checked against this run:
- An extension's `OnBefore<Op>`/`OnAfter<Op>` run on a trigger-firing operation: agrees, both
  apps.
- Field `OnBeforeValidate`/`OnAfterValidate` on `Validate`: not measured.
- The open gap on a symbol-only table when RunTrigger is not false: consistent with this run. It
  was not measured with a symbol-only app.

`src/program/table_event_guard.rs` says "Nothing here was probed on a live BC". It should be
updated when Windows confirms.

## Not measured

- Windows (pending).
- More than one row for `ModifyAll`/`DeleteAll`, and either one with no table-event subscriber
  (the bulk path).
- The global triggers (`OnDatabase<Op>`, `OnGlobal<Op>`).
- What orders two extensions' triggers, and a `Rec` change in `OnBeforeInsert` or
  `OnBeforeDelete`.
- `Rename`, and an extension's `OnBeforeRename`/`OnAfterRename`.
- `Validate`: field `OnValidate` and an extension's field `OnBeforeValidate`/`OnAfterValidate`.
  `Validate` has no RunTrigger argument.
- The validate events.
- Temporary records.
- Calls from a page, where `xRec` may differ.
- A dependency loaded from symbols only. BC runs the extension's compiled code either way; the
  question there is the engine's, not BC's.
