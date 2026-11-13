# ENG-25 probe: record operations written without parentheses in expression position

On the ENG-12 head `e53b26af` (master `26526e7c` + ENG-12; this code path is unchanged by ENG-12),
debug build, 2026-11-12. Workspace: table 52700 "E25 Row" (`OnModify` commits) and codeunit 52701
"E25 Ops" (the source: `ENG-25` tests, `tests/cli/cli_eng25_parenless_record_ops.rs`).

| Routine | Shape | Question | Answer | Right |
|---|---|---|---|---|
| "E25 Row".BareFind | `exit(FindFirst)` (implicit `Rec` in a table) | touches "E25 Row" down | no (every candidate decided) | yes |
| IfFind | `if R.FindFirst then;` | same | no | yes |
| NotEmpty | `exit(not R.IsEmpty)` | same | no | yes |
| ArrFind | `if Recs[1].FindFirst then;` | same | no | yes |
| WithFind | `with R do if FindFirst then;` | same | no | yes |
| ModifyValue | `exit(R.Modify)` | may-commit | no | yes: RunTrigger defaults to false, so `OnModify` does not run, but the table's `OnAfterModifyEvent` subscriber ("E25 Listener") commits |
| StmtFind (control) | `R.FindFirst;` (statement) | touches | yes | yes |
| WhileNext (control) | `if R.FindSet() then repeat until R.Next = 0;` | touches | yes (from `FindSet()`) | yes |
| FieldCount (control) | `exit(F.Count)`, `Count` a FIELD of "E25 Fieldy" | touches "E25 Fieldy" | no | no (a field read) |

Lowerer (`crates/al-syntax/src/lower/mod.rs` ~3577-3628): the statement form `R.FindFirst;` is
lowered to a Call with no arguments; the expression form stays a plain `Member` (no Call in the IR).
The extractor already emits a parens-less candidate site for a value-position member
(`extract::collect_value_v2`, `parenless: true`) and classifies `R.FindFirst` as a `RecordOp`; the
resolver's parens-less filter (`full.rs`) keeps a candidate only when its edge is a `Call` whose
every route reaches a routine, so a record operation (an `ImplicitTrigger` edge) is dropped, never
enters `parenless_calls`, and the body walk never makes it a record operation.
