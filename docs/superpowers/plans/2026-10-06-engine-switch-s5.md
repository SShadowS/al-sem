# Engine switch S5 — coverage, roots, ledger (G9, G10, G14)

Spec: `docs/superpowers/specs/2026-10-06-engine-switch-design.md` (rev 3), S5, plus two
items deferred into S5: the single derivation of object facts (S2b.4 note) and the ABI
object metadata the symbol-table move reads (S2b.5 note).
Branch: `engine-switch/s5-coverage-roots-ledger`, from master `dae85c39` (S4 merged).

## What exists

- Coverage on the analyze path (`gate::run::analysis_coverage`) already reads the
  program-backed model routines (S2b) and program calls (`calls_for`, S3); `opaqueApps`
  comes from the program's fresh coverage. Its source UNITS come from a separate disk
  walk that includes nested apps (`NestedApps::Walk`), while the model analyses only
  the root app's files (`NestedApps::Skip`, S2a): coverage can count files the model
  never analysed as parsed.
- Root classification runs over the model (program-assembled since S2b).
- `scope_filter` gets `|_obj_id| false`: no finding is ever treated as
  dependency-anchored.
- Diagnostics slot (2) "depArtifacts" is a tracked gap; `ProgramGraph::abi_ingest_errors`
  has no production reader.
- Object facts are derived twice from the same IR: `node_extract` (resolver's
  `ObjectNode`) and `ir_object_metadata` (model's `L3Object`).
- The adapter still infers receiver types with L3's `infer_receiver_type` over the
  model `SymbolTable` (S3 leftover, census and external-type naming only).

## Steps (one commit each, harness label per step)

- **S5.1 Coverage units = the files the model analysed** (G5/G9). The unit list uses
  the model's discovery policy (`NestedApps::Skip`). Test: a workspace with a nested
  app counts only the root app's files.
- **S5.2 App ledger and dependency-ingest diagnostics** (G14). A program-built ledger
  (primary app, declared dependencies with versions, resolved apps with source kind,
  ingest errors); `abi_ingest_errors` reach diagnostics slot (2).
- **S5.3 Dependency-role attribution in scope filtering.** The predicate asks the
  ledger whether the finding's object belongs to a non-primary app.
- **S5.4 One derivation of object facts** (G3/G4). Agreement census
  `node_extract` vs `ir_object_metadata` on every corpus first; then one derivation
  feeds both.
- **S5.5 Adapter receiver typing from the program engine.** Replace the leftover
  `infer_receiver_type` / model `SymbolTable` use in the adapter.

Each step: measure (`switch-baseline`, program stats where resolution could move),
triage, CHANGELOG, task gate.
