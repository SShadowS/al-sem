# Issue #241 — each routine's owner names its own declaring object

Spec: the `## Design` of `.agent/issue-241/ledger.md` (two review rounds, approved).

## Task 1 (the only task): owner by declaration

1. **Red.** Add A1-A6 to `tests/cli/cli_object_display.rs` (a `#if A codeunit 50100
   "Foo" { P } #else codeunit 50100 "Bar" { Q } #endif` workspace, A6 with the arms
   swapped). Add U1 to `src/engine/l4/effect_query_cli.rs`'s tests over hand-stated
   rows. Add `src/program/model/declaring.rs` with `DeclaringObjects::of` returning the
   last row (today's behaviour) so U1 compiles. Run: all seven fail for the stated
   reason.
2. **Green.** `DeclaringObjects::of`: no row -> `None`; one row -> it; several -> the
   row whose `source_anchor` is in the routine's file and contains its start; none ->
   the last row. Use it in `resolve_routine_selector` (both uses),
   `RoutineJoin` (field + `of`) and `OwnerIndex::build`. Rewrite the `RoutineJoin` and
   `OwnerIndex` doc comments.
3. **CHANGELOG** `[Unreleased]` `### Fixed`, with Limit L1' and the O5 sentence.
4. **Checks.** `scripts/check-goldens` (no golden may move), discrimination proofs for
   A1-A6 and U1 (each break made with Edit, asserted applied, reverted), then
   `bash scripts/ci-steps task`.

Not touched: `digest.rs`, `ordering_engine.rs`, `fingerprint_query.rs`,
`digest_cli.rs`, `fingerprint_cli.rs`, `prove.rs`.
