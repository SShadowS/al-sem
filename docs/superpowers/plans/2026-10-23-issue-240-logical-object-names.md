# Plan: #240 (+ #249) logical object and table names

Spec: the `## Design` of the issue-240 ledger (spec rounds 1-3, both reviewers
approved). Base `7c35891a`.

## Task 1: the model, its producers and its lookups hold and match logical names

- `ir_walk::ir_object_name` (one helper); the model (`workspace.rs`), R0
  (`engine/snapshot.rs`) and L2 (`l2_workspace.rs`) take the name from it.
- `symbol_table::NameRef { Raw, Logical }` on `SymbolTable::object_by_type_name`,
  `SymbolTable::table_by_name`, `extension_fields::table_index_by_name` and
  `Model::table_by_name`; every caller states its provenance (the caller table),
  the owner's through `ModelObject::name_origin()`.
- `ir_walk::parse_record_table_name` keeps a quoted name whole (an escaped `""` does
  not end it). Not in the spec: found while writing T9 (see the ledger's
  Implementation section).
- Source-text displays (`table_display` tier 2, d52) and the object-fact census
  unescape once.
- Tests: T1-T4, T9 (`tests/cli/cli_object_display.rs`), T5-T8
  (`tests/cli/cli_escaped_object_names.rs`), T10 (`census.rs`), and the two #83
  display assertions moved to the logical name.

## Task 2: docs and CHANGELOG

- `unescape_al_identifier`'s caller list; `record_types` module doc.
- CHANGELOG `Fixed` entry with the hash, `alsem diff`, dependency-resolution,
  selector and `interfaceName` notes.

## Verification

`scripts/check-goldens` (expect no move), every discrimination proof in the ledger
table, `bash scripts/ci-steps task`.
