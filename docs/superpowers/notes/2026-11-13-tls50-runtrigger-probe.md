# TLS-50: what `RunTrigger = false` runs (BC probe)

**Status: measured on BC 28 Linux, Windows pending.** Per CLAUDE.md, a sidecar fact becomes an
engine fact only after a Windows Cronus confirms it.

## Question

Does `Insert(false)`, `Modify(false)` or `Delete(false)` (and `ModifyAll(…, false)`,
`DeleteAll(false)`) run any table trigger (`On<Op>`) or table-extension trigger
(`OnBefore<Op>`, `OnAfter<Op>`)? Do the table events (`OnBefore/OnAfter<Op>Event`) run?
ENG-15's symbol-only gap and the resolver's RunTrigger rule assume no trigger and every event
(`resolver::table_event_names`'s doc). Asked for ENG-71: what does `Rec` hold in a table
extension's `OnBefore<Op>`/`OnAfter<Op>`?

## Environment

- BC 28.4 on the kraken Linux sidecar (`http://bc`, instance `BC`, tenant `default`).
  Platform (System) 28.0.54946.56477; Application 28.4.53241.56590.
- Compiled with alc 18.0.41.62505 (`/opt/al-tool/al compile`); published at Dev scope with
  `bcdev_app_publish`; run with `bcdev_test_run`.
- Probe source: `docs/superpowers/notes/tls50-probe/` (`app.json`, `src/Probe.al`).

## The probe

All objects are in one app:
- table `TLS50 Item` (`PK`, `Val`) with `OnInsert`, `OnModify` and `OnDelete`;
- a `tableextension` of it with `OnBefore`/`OnAfter` `Insert`, `Modify` and `Delete`;
- subscribers to the six table events.

Each one appends a line to a log table:
- the source: `T.` is the table, `X.` the extension, `E.` an event;
- `rec`: `Rec.Val`;
- `x`: `xRec.Val`, for triggers;
- `rt`: the event's `RunTrigger`;
- `db`: the stored row's `Val`, `-1` when no row is stored.

There is one `[Test]` method per operation and argument. Each starts from a row seeded with
`Val = 1`; Insert starts from none. It sets `Val` and calls the operation. The test then raises
the log as its error, which is how the result leaves the test, so every test reads "failed" by
design.

## Raw results (2026-11-13, one run)

| Test | Log, in order |
|---|---|
| InsertFalse | E.OnBeforeInsertEvent rt=No rec=5 db=-1 \| E.OnAfterInsertEvent rt=No rec=5 db=5 |
| InsertTrue | E.OnBeforeInsertEvent rt=Yes rec=5 db=-1 \| X.OnBeforeInsert rec=5 x=5 db=-1 \| T.OnInsert rec=5 x=5 db=-1 \| X.OnAfterInsert rec=5 x=5 db=5 \| E.OnAfterInsertEvent rt=Yes rec=5 db=5 |
| ModifyFalse | E.OnBeforeModifyEvent rt=No rec=2 db=1 \| E.OnAfterModifyEvent rt=No rec=2 db=2 |
| ModifyTrue | E.OnBeforeModifyEvent rt=Yes rec=2 db=1 \| X.OnBeforeModify rec=2 x=2 db=1 \| T.OnModify rec=2 x=2 db=1 \| X.OnAfterModify rec=2 x=2 db=2 \| E.OnAfterModifyEvent rt=Yes rec=2 db=2 |
| DeleteFalse | E.OnBeforeDeleteEvent rt=No rec=9 db=1 \| E.OnAfterDeleteEvent rt=No rec=9 db=-1 |
| DeleteTrue | E.OnBeforeDeleteEvent rt=Yes rec=9 db=1 \| X.OnBeforeDelete rec=9 x=9 db=1 \| T.OnDelete rec=9 x=9 db=1 \| X.OnAfterDelete rec=9 x=9 db=-1 \| E.OnAfterDeleteEvent rt=Yes rec=9 db=-1 |
| ModifyAllFalse | E.OnBeforeModifyEvent rt=No rec=3 db=1 \| E.OnAfterModifyEvent rt=No rec=3 db=3 |
| ModifyAllTrue | E.OnBeforeModifyEvent rt=Yes rec=3 db=1 \| X.OnBeforeModify rec=3 x=3 db=1 \| T.OnModify rec=3 x=3 db=1 \| X.OnAfterModify rec=3 x=3 db=3 \| E.OnAfterModifyEvent rt=Yes rec=3 db=3 |
| DeleteAllFalse | E.OnBeforeDeleteEvent rt=No rec=1 db=1 \| E.OnAfterDeleteEvent rt=No rec=1 db=-1 |
| DeleteAllTrue | E.OnBeforeDeleteEvent rt=Yes rec=1 db=1 \| X.OnBeforeDelete rec=1 x=1 db=1 \| T.OnDelete rec=1 x=1 db=1 \| X.OnAfterDelete rec=1 x=1 db=-1 \| E.OnAfterDeleteEvent rt=Yes rec=1 db=-1 |

## Findings (this shape: one row, triggers and extension in one app, code-driven calls)

1. **`RunTrigger = false` runs no table trigger and no table-extension trigger**, `OnBefore` and
   `OnAfter` included. This holds for `Insert`, `Modify` and `Delete`, and for `ModifyAll` and
   `DeleteAll`.
2. **The table events run in every case**, with `RunTrigger` passed through (`No`/`Yes`). This
   agrees with `resolver::table_event_names`'s doc. With a subscriber present, `ModifyAll` and
   `DeleteAll` raised the events for their one row.
3. **Order with `true`:**
   1. `OnBefore<Op>Event`;
   2. extension `OnBefore<Op>`;
   3. table `On<Op>`;
   4. the write;
   5. extension `OnAfter<Op>`;
   6. `OnAfter<Op>Event`.
4. **What `Rec` holds (for ENG-71).**
   - In the extension's `OnBefore<Op>` and the table's `On<Op>`:
     - `Rec` holds the caller's values (the new `Val`);
     - the stored row is still the old one (`Modify`, `Delete`), or there is none (`Insert`).
   - In the extension's `OnAfter<Op>`:
     - `Rec` holds the same values;
     - the write is done: `db` is the new value, or `-1` after a delete.
   - `xRec` equals `Rec` in every trigger here. The call came from code, not a page, so `xRec`
     is not the stored row.
   - The events see the same values: `OnBefore<Op>Event` before the write, `OnAfter<Op>Event`
     after it.

**The engine's rule agrees, so no code changes.** RunTrigger false gives no trigger edge, and
every table event is raised.

## Not measured

- Windows (pending).
- A table extension in another app than its table.
- A `tableextension`'s field `OnBeforeValidate`/`OnAfterValidate`. `Validate` has no RunTrigger
  argument.
- `Rename`.
- More than one row for `ModifyAll`/`DeleteAll`.
- Temporary records.
- Calls from a page, where `xRec` differs.
