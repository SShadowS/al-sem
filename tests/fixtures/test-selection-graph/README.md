# test-selection-graph fixture

The `alsem graph` fixture (#63, spec
`docs/superpowers/specs/2026-10-13-callgraph-export-design.md` §10.3). The first
part is a trimmed copy of bc-dev-mcp's `tests/fixtures/test-selection-acceptance`;
the rest adds one case per may-execute rule and per open-site hub.

| File | What it pins |
| --- | --- |
| `Loyalty.al` | an event (`OnAfterCalcPoints`) and a `Codeunit.Run` chain (`RecalcNightly` -> `Acc Nightly Recalc.OnRun` -> `ApplyDecay`) |
| `Tax.al` | interface dispatch over three implementations |
| `Invoice.al` | an event whose subscriber is reached only through it |
| `LegacyExport.al` | `ExportLine`, which no test reaches |
| `Ledger.al` | a table with `OnInsert` and a field `OnValidate`, its card page (`OnAfterGetRecord`, a field `OnValidate`, an action), a report, and a subscriber to the table's `OnAfterInsertEvent` (no record operation raises it yet: `unraisedPublisher`) |
| `Dynamic.al` | open sites: `Codeunit.Run(Id)` (run hub), a Variant member call (name hub), `RecordRef.Insert(true)` (record hub), `Report.SaveAsXml(Report::X, ...)` (run hub), an unbound subscriber |
| `Tests.al` | one test per case: a `TestPage`, a modal page handler, a record insert, the dynamic sites |

Ids 50900-50999. Nothing here is compiled; the engine reads the source only.
