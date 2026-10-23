# Plan: #240 (+ #249) logical object and table names

Spec: the `## Design` of the issue-240 ledger (spec rounds 1-3, both reviewers
approved). Base `7c35891a`.

## Task 1: the model, its producers and its lookups hold and match logical names

- `ir_walk::ir_object_name` (one helper); the model (`workspace.rs`), R0
  (`engine/snapshot.rs`) and L2 (`l2_workspace.rs`) take the name from it.
- Superseded in final round 1 (CR3): the first cut put `NameRef { Raw, Logical }`
  on the four lookups. It is replaced by **normalize at production**: every
  reference the model stores is unescaped once where it is produced (the model's
  `SourceTable` / `extends`; `ir_walk`'s record-variable table, `TableNo` (one
  quote pair, CR1), dataitem table and `Codeunit.Run` target;
  `record_types::record_table_name_of`; the ABI projection's quoted `SourceTable` /
  `TargetObject`). The lookups take logical `&str` again.
- `ir_walk::parse_record_table_name` keeps a quoted name whole (an escaped `""` does
  not end it). Not in the spec: found while writing T9 (see the ledger's
  Implementation section).
- The object-fact census unescapes the graph side once (the graph stays raw, #250);
  the displays read the stored logical name.
- Tests: T1-T4, T9 (`tests/cli/cli_object_display.rs`), T5-T8b
  (`tests/cli/cli_escaped_object_names.rs`), T10 (`census.rs`), unit tests in
  `record_types` and `symbol_reference`, and the two #83 display assertions moved to
  the logical name.

## Task 2: docs and CHANGELOG

- `unescape_al_identifier`'s caller list; `record_types` module doc.
- CHANGELOG `Fixed` entry with the hash, `alsem diff`, dependency-resolution,
  selector and `interfaceName` notes.

## Verification

`scripts/check-goldens` (expect no move), every discrimination proof in the ledger
table, `bash scripts/ci-steps task`.
