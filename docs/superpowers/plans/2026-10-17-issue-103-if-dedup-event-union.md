# Issue #103: union event facts across `#if`-duplicated routines (plan)

The spec is in `.agent/issue-103/ledger.md`: D1, D1b, D2, the acceptance matrix A1-A8, and discrimination breaks 1-8. The spec panel converged at round 5.

## T0: acceptance tests (red)

Write these tests and confirm each fails for the stated reason. Nothing is committed alone.

- **`build.rs` unit tests:** A1, A1b, A1c (union), A3/A3b (publisher facts), A6/A6b/A6c (no leakage).
- **CLI tests in `tests/cli/cli_attribute_trivia.rs`:** A2 (FireB reaches COMMIT) and A3 (the `ws3` edge).
- **Program-engine test:** A2b (the OnB edge is `resolvedSource`, not `honestEmpty`).
- **Model test:** A5 (exactly one edge per subscription).
- **Verifier test:** A7, including the fixed `semantic_golden` test and its twin. The `semantic_golden` fix lands with T1 if it needs the new matching.
- **Cross-app test:** A8.

## T1: implementation

- D1: the union, limited to the source tier and the same key, with the ordinal doc updated.
- D1b: the verifier matches on the full id and loops over all declarations.
- D2: the row skip, applied after mapping, with the census as decided.
- Run the discrimination breaks 1-8.

## T2: measurement and close-out

- Run the D4 CDO probe, then remove it.
- Check that the CDO histogram is byte-identical between master and the branch.
- Run `check-goldens`.
- Add the CHANGELOG entry.
