# Issue #302 — ABI ingestion reads page write-surface properties (plan)

Spec: `## Design` (revision 3) in the issue ledger. This plan only orders the work.

A dependency page known only from its symbols (no embedded `.al`) must carry
`Editable`, `InsertAllowed`, `ModifyAllowed`, `DeleteAllowed` and
`SourceTableTemporary`, so d39's read-only rule and opt-in d64 judge it like a
source page. Symbols spell the values `"0"` / `"1"`.

## Task 1 — parse the five properties (symbol_reference.rs, projection.rs)

1. Add `AbiPageProps` (five `Option<bool>`) as `AbiObject.page_props`, and
   forward it as `ProjectedObject.page_props`. Do not fill it yet.
2. Write test 1 in `symbol_reference.rs`'s tests: the table-driven value contract
   (`"1"`, `"0"`, `"true"`, `"false"`, `"TRUE"`, `" False "`, `"maybe"`, `""`,
   absent) and the three-page field wiring (`Wire A/B/C`, pairwise-distinct
   patterns), a mixed-case property name, and a codeunit that keeps the default.
   Run it: red (every field still `None`).
3. Add `abi_bool` next to `raw_object_property`, with a doc saying why its rule
   differs from the native `== "true"` rule, and fill `page_props` inside the
   existing `if object_type == "Page"` block. Run test 1: green.
4. Breaks B3a–B3g, B7, B8 against test 1. Record each in the ledger.
5. Gate: `scripts/ci-steps task-affected`, `scripts/check-goldens` (no regen, no
   golden may move), commit, `agentflow check-diff`.

## Task 2 — carry them into the model, CLI tests, docs

1. Fixture `tests/fixtures/issue-302/`: the dependency's `SymbolReference.json`
   and its `.al` twin, and the workspace (`app.json`, `src/`). The test writes the
   `.app` into a temp copy with `symbol_app::write_symbol_app` /
   `write_source_app` (fixed zip timestamp, so the bytes are deterministic).
2. Tests 2 and 3 in `tests/cli/cli_d39_dependency_page.rs` (d39 symbol-only vs
   embedded twin, primary scope; opt-in d64 at all scope), with hand-stated
   preconditions (no `.al` in the zip, each property literal once, `opaqueApps`,
   the stderr warning). Run: red (`abi_rows` still writes `None`).
3. Map the five fields in `abi_rows::dep_object_to_l3`. Run: green.
4. Doc fixes: `mod.rs` (`implicit_rec_persisted_by_owner` main limit,
   `page_may_modify`, add the temp-base-page FP limit), `d39.rs` module doc,
   `workspace.rs` `ModelObject` field docs. CHANGELOG `[Unreleased]` → `Fixed`.
5. Breaks B1, B2, B3b/B3g (CLI side), B4a, B4b, B5, B6. Record in the ledger.
6. Gate as in Task 1, then the final-head CDO fingerprint gate (release-fast,
   base vs head, `--scope primary` and `--scope all`, 0 moves each).
