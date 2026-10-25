# Issue #299 plan: d11 and d37 skip the table receiver `Rec`

Spec: the `## Design` section (revision 3) of `.agent/issue-299/ledger.md`. The
worktree starts from the revision-3 prototype (`.agent/runs-local/proto/proto-r3.diff`).

## T1: the rule and its lib test

- `PRecordVariable.is_receiver` (`#[serde(skip)]`, manual `PartialEq` over the
  serialized fields), set only on the seeded table / tableext `Rec` row with no named
  return `Rec`; forwarded to `ModelRecordVariable.is_receiver`.
- `mod.rs`: `is_table_receiver_rec`, `table_rec_persisted_by_owner` (extracted from
  `implicit_rec_persisted_by_owner`), `receiver_reset_after`.
- d11 and d37: the receiver skip as the last check, stats key `callerOwnedRec`.
- Lib test `receiver_flag_is_not_serialized_and_not_compared`; the INTERNAL-ONLY list
  gains `PRecordVariable.is_receiver` (count 10). Red: `is_receiver` in `eq`.
- `readers_see_the_implicit_rec` gets its post-change rows (the rule moves them, so
  it belongs with the rule).
- Gate: `ci-steps task-affected`, `check-goldens`, `check-diff`. One commit.

## T2: the CLI fixture and test, the old test, docs, CHANGELOG

- Fixture `tests/fixtures/issue-299/ws-owned` (from the dry-run fixture; comments
  match the rows).
- `tests/cli/cli_caller_owned_rec.rs` (`mod` in `tests/cli/main.rs`): the exact 27
  rows through `run_analyze_with_exit`. Red: the base source.
- d11 / d37 module docs (S11, OP7); doc pointers in `mod.rs`.
- CHANGELOG `[Unreleased]` / `### Changed`.
- Discrimination proofs 1-7 of the Design, recorded in the ledger.
- Gate as T1. Then the final-head fingerprint gate (ledger "Final-head measure").
