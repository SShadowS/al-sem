# ENG-24 probe: what d50 misses before a checked Codeunit.Run

On master `db06b145`, debug build, 2026-11-13. d50 is opt-in:
`alsem analyze <ws> --detector d50-checked-run-implicit-commit --format json --scope all`.
Workspace: table 53000 "E24 Row", codeunit 53001 "E24 Worker" (empty `OnRun`), codeunit 53002
"E24 Ops" (each routine below ends in `if Codeunit.Run(Codeunit::"E24 Worker") then;`), codeunit
53003 "E24 Subs" (subscribers).

| Routine | Before the checked run | d50 | Right |
|---|---|---|---|
| PlainWrite (control) | `R.Get(1); R.Modify();` | finding | finding |
| RecRefWrite | `RR.Open(Database::"E24 Row"); RR.FindFirst(); RR.Modify();` | finding (the literal `Open` resolves the table) | finding |
| RecRefParam | `RR.Modify();` on a `var RR: RecordRef` parameter (table not known) | **none** | finding |
| LockThenRead | `R.LockTable(); R.FindFirst();` | **none** | finding (`LockTable` then a read opens a write transaction, ENG-22, confirmed on Windows) |
| SubscriberWrites | `T.Insert(true)` on a temporary record; the table's `OnAfterInsertEvent` subscriber modifies a real row | finding (the subscriber's cone is counted) | finding |
| "E24 Subs".RunsCheck | sibling subscriber of `OnSomething`: the other subscriber (`WritesFirst`) modifies a row; their order is not known | **none** | finding, labeled "order unknown" |

For contrast, `prove … throws-error` (the trap search, ENG-22) answers `yes` for RecRefParam and
LockThenRead. For `RunsCheck` it answers `no` from that routine as root (its own reach has no
write), and for `Raise` `yes`. The sibling shape is the one whose order the engine does not know.

Cause: d50 reads `TransactionSpan::pending_physical_tables` (`src/engine/l5/pending_writes.rs`),
whose effects count only writes to a known physical table and published events. The trap search's
rule (`literal_reach.rs` `build_pending_writes`) also counts a write whose table is not known, a
`LockTable`, and unknown code; neither reader counts a sibling subscriber.
