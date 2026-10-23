# Plan: #253 per-`[Test]` quality block in `--analyze --format json`

Spec: the `## Design` (v3 + round-4 note) in the issue ledger. Acceptance tests
(`tests/cli/cli_analyze_test_quality.rs`, fixtures `tests/fixtures/analyze-test-quality{,-noapp}/`)
are written first and seen red: no `test_quality` key.

## Task 1: body-only metrics

- `src/test_quality.rs` (new, `pub mod test_quality;` in `lib.rs`): row/sum structs, the
  exhaustive IR walker (no catch-all arm), the receiver lookup (locals, params, globals;
  `this.X` in globals; type by `classify_type_text`), the literal classifier, unit test U1
  (hand-built `BlockItem::Preproc`) and one classifier unit test per shape.
- `src/analysis.rs`: `file_metrics` returns, per routine with a body, its own count keyed by
  `(path, byte start)` and a test row per `[Test]` routine, from the same loop as `metrics`.
  `analyze_project` joins them in file order and builds `codeunits` and `workspace`;
  `AnalysisResult.test_quality` is the last field.
- `tests/cli/cli_analyze_call_hierarchy_json.rs`: the two legacy-test edits (drop the
  `test_quality` key; stop the raw key scan at `"findings": [`).

## Task 2: reachable count

- `analysis.rs`: an app-root helper (deepest folder with an `app.json`, else the project root),
  replacing `in_nested_app`.
- `test_quality.rs`: the call map keyed by (caller app root, `RoutineNodeId`) from
  `edges_by_file` (Call/Run/ImplicitTrigger), the per-(root, id) own count summed over
  declarations, and the per-test worklist walk with a visited set seeded with the test's id.
  `null` without a graph or a declaration, propagated into sums.

## Then

Discrimination proofs (ledger table), `scripts/check-goldens`, CHANGELOG + README line, CDO
measurement (master vs branch, release-fast, `--format json`), `scripts/ci-steps task`.
