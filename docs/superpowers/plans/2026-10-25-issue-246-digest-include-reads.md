# Issue #246 plan: `alsem digest --include-reads`

The design is the `## Design` section of `.agent/issue-246/ledger.md`. The spec panel accepted it in round 2. The issue is bounded and has two tasks, so per the flow no per-task reviewer runs. The implementer's TDD and discrimination proofs plus the final panel stand in for that review.

Global constraints (both tasks):
- **Default output stays byte-identical.** Without the flag, no golden may move.
- **Do not touch:** `.github/`, `scripts/`, `.claude/`, `CLAUDE.md`, `.gitignore`, the `Cargo.toml` version, `tree-sitter-al`, `.agent/` (the ledger is the one exception).
- **Code style:** `rustfmt --edition 2024 <file>`. Edit and Write only. No `#[allow]` or `#[ignore]`.
- **Gate:** `bash scripts/ci-steps task-affected`, `bash scripts/check-goldens`, then `agentflow check-diff`.

## Task 1: the flag, `DB_READ`, the ordering exclusion, the tests
- **Flag path.** `DigestCli.include_reads` → `DigestRequest` → `digest_plan` → `TreeDemand` → `digest_effects` → `compute_digest_effects_cli` → `digest_query`/`digest_query_with` → `digest_one_root`. Every other caller passes `false`; the call sites are listed in the Design (OQ5).
- **`DB_READ`.** In `digest_one_root`, behind `include_reads &&`, a table fact with `op == "read"` becomes `DB_READ`. `map_op` itself is not changed.
- **Ordering exclusion.** `DB_READ` is left out of `ordering_inputs`, then `scoped` is remapped per position. This applies only when the flag is on.
- **`tempState`.** The CLI join gains a `DB_READ` arm.
- **Payload echo.** `payload.includeReads: true` is written only when the flag is set.
- **CLI help.** The help text states the documented limits and that `DB_READ` is "not graded".
- **Tests T1–T5, per the Design.** Each test drives the `alsem` binary or `run_digest_pipeline` and has its own discrimination proof.
  - T2 uses the exact 4-step normalization and asserts more than 0 `DB_READ` in the corpus. Record the fixture that T2's remap break fails on, or record that only T4 catches it (O10).
  - T3 asserts `tempState`, transitive provenance and `resourceId`.
  - T4 checks the ordering exclusion with two separate breaks.
  - The A3 test gets a break that targets reads specifically.
- Commit: `#246 Task 1: digest --include-reads (DB_READ)`.

## Task 2: CDO measurement, admission row, CHANGELOG
- **Runs.** Use #277's exact `d`, `h`, `ds` and `Up` argv, each with and without `--include-reads`. Build one release-fast binary and take peaks with `/work/bench/160-10-11a/maxrss.py`. Do 3 alternating runs on a quiet box; coordinate with the other sessions first.
- **Record:** median and range, output bytes, the `DB_READ` count, and the number of `DB_READ` temp-state-incoherent diagnostics.
- **Admission rule** (fixed before measuring):
  - If a reads median is above the `("digest", X)` or `("digest", S)` row, raise that row in its own commit, citing the numbers.
  - If any reads peak is above 7,340,032 KiB, STOP and report to the operator.
- **Outputs.** Write the numbers file `docs/measurements/2026-10-25-issue-246-digest-include-reads.md`. Add a CHANGELOG `Added` entry with the measured cost (acceptance 4), the documented limits and the schema note.
- Commit: `#246 Task 2: the --include-reads cost on CDO; admission row; CHANGELOG`.
