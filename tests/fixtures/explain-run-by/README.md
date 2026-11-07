# explain-run-by fixture

What explain says about who calls a routine and what runs a trigger
(round-2 output validation, `docs/superpowers/specs/2026-11-07-output-validation-r2.md`
§2.3 G3 and §2.4 G4's W1 rows). `tests/cli/cli_explain_run_by.rs` states each
expectation by hand from this text.

| Routine | What explain must say |
| --- | --- |
| `RB Mgt.Target` | 3 direct callers: `Middle` and `ProcB` (routines) and the `Direct` action's `OnAction` (a page action); 2 page actions reach it in all (`Direct` directly, `Indirect` through `Middle`) |
| `RB Ledger."No."::OnValidate` | run by `Validate` calls on the field: 1 site (`RB Mgt.Validator`); potential UI surfaces: 1 page on its table (`RB Ledger Card`) |
| `RB Ledger.Amount::OnLookup` | no code call runs it; the same page |
| `RB Actions.action(Direct)::OnAction` | an entry point: the action itself, no code caller |
| `RB Subscriber.OnAfterInsertLedger` | run by its publisher, `Table "RB Ledger"::OnAfterInsertEvent`; no direct code caller |
| `RB Ledger::OnInsert` | no direct code caller; what runs a table trigger is not listed yet (W3) |

Ids 51200-51299. Nothing here is compiled; the engine reads the source only.
