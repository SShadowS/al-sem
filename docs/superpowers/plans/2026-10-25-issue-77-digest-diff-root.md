# Issue #77 digest --diff-root Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** `digest --diff` resolves a diff made at the enclosing git top level like a workspace-relative one; `--diff-root <dir>` names the base folder explicitly; a path naming two workspace files is reported `diff-file-ambiguous`, never picked.

**Architecture:** The CLI decides one `DiffRoot` per job (`Workspace` / `GitTop(prefix)` / `Explicit(prefix)`) in `decide_diff_root`, pinned by a batch parent and carried to its worker (`WorkerJob.diff_root`). `resolve_changed_roots` reads every diff path through `diff_readings` against the workspace's FILE set (`al_files`), so lookup, ambiguity and the #72 block share one reading rule.

**Tech Stack:** Rust, clap, serde; tests through `CARGO_BIN_EXE_alsem` (cargo-nextest).

**Spec:** `.agent/issue-77/ledger.md` `## Design` (revision 2.3, converged spec panel r5). Prototype `/work/bench/77/rev2/proto-rev2.patch` is the reference for code shape, NOT for case folding.

## Global Constraints

- Prefix comparison is exact on every platform (no case fold); below the workspace the existing lowercased key stays.
- Digest `schemaVersion` "1.7.0"; the version comment states: a minor bump is additive, `changedRootsDiagnostics[].kind` is an open set. Prove does not bump.
- Auto mode never fails (canonicalize failure → `Workspace`); `--diff-root` failures and a lone `--diff-root` are exit 3 (`exit::CONFIG_ERROR`).
- `--diff-root` must exist and be a directory, checked before either containment test.
- `--diff-root` help text exactly as Design Changes 5.
- No change to `diff_parser.rs`, `--file`/`--changed-files`, human output; no `git` subprocess.
- Do not touch `.github/ scripts/ .claude/ CLAUDE.md .gitignore`, the Cargo.toml version, `tree-sitter-al`; `.agent/` is never committed. Edit/Write only; `rustfmt <file>`.

## Review Focus

- A diff path equal to the prefix itself (`Cloud`, `Cloud/`) must name nothing, not the workspace root — pinned by A24 (`strip_diff_prefix` → `None`).
- A `--diff-root` spelled relative to the current directory — resolved with `std::path::absolute` on both sides (A2/A6 pass absolute; relative spelling is review-only).
- Library callers (`run_digest_pipeline`, `digest_pipeline_with`) keep `Workspace` and still match diffs — the existing digest goldens and #71/#72/#111 tests pin it.
- A batch job's `.git` probes are recorded after the `PLAN_TOUCHED` clear so they join that job's reads — A21 (parent = worker = standalone).
- An ambiguous broken record never leaks roots from either candidate — A12.

---

### Task 1: diff-root decision, readings, ambiguity, schema 1.7.0

**Files:**
- Modify: `src/engine/l5/digest_cli.rs` (`normalize_input` ~:113 split into `clean_input`; `ChangedRootsDiagnostic` ~:190; `ChangedInput` ~:228; `resolve_changed_roots` diff section ~:460-525; `changed_roots_diagnostic_to_value` ~:1416; `schemaVersion` ~:1573; `DigestRequest`, `run_digest_pipeline`, `DigestJob::changed_input`; unit tests)
- Modify: `src/engine/l5/prove.rs` ~:940 (`diff_root: Default::default()`, `workspace_files: Vec::new()`)
- Modify: `src/engine/batch/worker.rs` `WorkerJob` (+`diff_root`)
- Modify: `src/bin/alsem.rs` (`DigestCli.diff_root`, `diff_root_decided` `#[arg(skip)]`; `digest_has_diff`, `decide_diff_root`, `pin_diff_root`; early exit-3; batch parent ~:1066 and worker ~:1186)
- Modify: `CHANGELOG.md` `[Unreleased]`, `README.md` (one line under the digest examples)
- Test: `tests/cli/cli_digest_changed_spans.rs` (A1–A22, written in step 6), three `schemaVersion` asserts (A23), unit tests A8b/A24 in `digest_cli.rs`
- Goldens: 22 digest/batch goldens move only their `schemaVersion` line (regen)

**Interfaces:**
- Produces: `pub enum DiffRoot { Workspace, GitTop(String), Explicit(String) }` (Default `Workspace`, Serialize/Deserialize); `DiffRoot::below(rel: &Path, explicit: bool) -> DiffRoot`; `fn same_folder(a: &str, b: &str) -> bool` (`a == b`); `fn strip_diff_prefix(prefix: &str, path: &str) -> Option<String>`; `fn diff_readings(files, root, path) -> Vec<String>`; `fn diff_key(spans, files, root, path) -> Result<String, ChangedRootsDiagnostic>`; `ChangedRootsDiagnostic::DiffFileAmbiguous { file, candidates }`; `ChangedInput.{diff_root, workspace_files}`; `DigestRequest.diff_root`; `WorkerJob.diff_root: Option<Result<DiffRoot, String>>`.

- [ ] **Step 1: Unit tests A8b, A24** in `digest_cli.rs` `mod tests`: `same_folder("Cloud","cloud")` false, `same_folder("Cloud","Cloud")` true; `strip_diff_prefix("Cloud","Cloud/src/x.al") == Some("src/x.al")`, `("Cloud","Cloudy/x.al")`, `("Cloud","Cloud")`, `("Cloud","Cloud/")`, `("Cloud","cloud/src/x.al")` → `None`, `("a/b","a/b/X.al") == Some("x.al")`.
- [ ] **Step 2: Red**: `cargo nextest run --test cli -E 'test(/cli_digest_changed_spans::/)'` and the lib tests — the step-6 rows fail as the ledger Timeline records.
- [ ] **Step 3: Implement** the Interfaces above per Design "How a diff path is read", "Ambiguity and the #72 veto", "Decision pinning"; existence/is-dir check of `--diff-root` first (message names `--diff-root`); `decide_diff_root` doc names `pin_diff_root`; `plan_digest` uses `digest_has_diff` for its own `has_diff`.
- [ ] **Step 4: Green**: `scripts/ci-steps task-affected` exit 0.
- [ ] **Step 5: Discrimination proofs**, one per row with the matrix's break (Edit, run, fail; revert, run, pass); a row each in the ledger.
- [ ] **Step 6: Goldens**: `scripts/check-goldens --regen`; `git diff --stat` shows exactly the 22 expected files, every hunk a `schemaVersion` line; then `scripts/check-goldens` green. Triage in the ledger.
- [ ] **Step 7: Docs**: CHANGELOG Added/Changed per Design Changes 6 (incl. qualifying the #111 lines); README line.
- [ ] **Step 8: Commit** after `agentflow check-diff` prints no reason: stage the named files only.
