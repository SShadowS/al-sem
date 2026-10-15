# Issue #70 — cross-app follow-ups from #66 — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** `events` and `policy check` load dependencies by default; `prove`/`fingerprint`/`diff` stay
single-app with a written reason; `workspaceFingerprint` covers the loader's whole dependency closure;
a dependency event publisher's dispatch reaches the cross-app L4 store with conservative parameter roles.

**Architecture:** Reuse #66's cross-app base (`build_cross_app_base`, `R3a5CrossAppBase`,
`LoadedDependencies`). One discovery (`dependencies::find_all_alpackages_folders`, git-boundary fixed)
feeds the loader, the closure digest and the fingerprint. One leaf predicate plus a `fixed_roles` set in
`build_cross_app_base`, carried to every cross-app solve through `CrossAppInputs`.

**Tech Stack:** Rust; `alsem` CLI (clap); integration tests in the `tests/cli` umbrella.

**Spec:** `docs/superpowers/specs/2026-10-15-issue-70-cross-app-follow-ups-design.md` (converged,
5 rounds) and its probes `…-probes.md`. Read the spec's Design and Acceptance matrix before any task;
each task below names the matrix rows it turns green.

## Global Constraints

- No `#[allow(`; `rustfmt <file>` per touched file, never `cargo fmt`; Edit/Write tools only.
- Never touch `.github/ scripts/ .claude/ CLAUDE.md .gitignore Cargo.toml tree-sitter-al .agent/`.
- Every cargo command in the worktree with `TREE_SITTER_AL_PATH=U:/Git/al-call-hierarchy/tree-sitter-al`
  and `CDO_WS` unset.
- A golden move is triaged line by line before any regen (golden-diff-triager); only the moves the spec's
  "Golden families expected to move" lists may be blessed.
- Every new/changed test gets a discrimination proof with its matrix row's mutation; record in the ledger.
- The acceptance tests already exist (step 6, uncommitted); do not weaken them.

## Review Focus

1. A workspace with NO dependency and NO `platform`: events/policy output must be byte-identical to today
   apart from the new `dependencies` key (1f).
2. A missing primary app (no `app.json`): events/policy must error and name `--single-app` (as digest).
3. CDO: `events`/`policy` cross-app cost and new findings — measured, every new policy finding triaged.
4. The leaf predicate must not move any analyze golden; a move is a real dispatch row to triage.
5. Fingerprint values in the 171 golden files must not move (zero churn).

---

### Task 1: one dependency discovery (git boundary, closure entries, fingerprint)

**Files:** `src/dependencies.rs` (`find_all_alpackages_folders`, `discover_app_files`),
`src/program/resolve/semantic_golden.rs` (`dependency_closure_digest` → `dependency_closure_entries`),
`src/engine/l5/snapshot_full.rs` (`derive_inputs`). Rows: 2a, 2b, 2c, 2e, 2f, 2g, 2h.

**Interfaces:** Produces `pub struct ClosureEntries { pub entries: Vec<(String, PathBuf)>, pub errors: Vec<(String, ClosureErrorKind)> }`,
`pub enum ClosureErrorKind { ListingFailed, ReadFailed }` (stable `as_str()`: `listing-failed` / `read-failed`),
and `pub fn dependency_closure_entries(root: &Path) -> ClosureEntries`; `dependency_closure_digest` keeps
its signature and output, built on top of it (returns `Err` when `errors` is non-empty).

- [ ] Run the rows' tests; confirm red per `red.md`.
- [ ] `find_all_alpackages_folders`: if `project_root.join(".git").exists()`, return only the own cache.
- [ ] Extract `dependency_closure_entries` (same keys, same sort, same canonical dedup); errors keyed by the
      same `../`-relative key; never drop readable entries.
- [ ] `derive_inputs`: one `dep-package` input per entry (`path = key`, `contentHash = sha256(bytes)`), one
      `dep-package-unreadable` per error (`contentHash = sha256(kind.as_str())`); remove its own `read_dir`.
- [ ] Tests green; `--test cli`, `--test differential`, `--lib` green; `check-goldens` zero diff.
- [ ] Proofs per row; `check-diff`; commit `feat: one dependency discovery feeds the loader, closure digest and fingerprint (#70)`.

### Task 2: dependency publishers reach the cross-app store, with conservative roles

**Files:** `src/engine/l4/capability_cone.rs` (`build_cross_app_base` leaf predicate ~:3403-3416,
`R3a5CrossAppBase`, `cross_app_inputs()`, the base's own solve ~:3430-3493, doc ~:3293-3296),
`src/engine/l5/detector_context.rs` (`CrossAppInputs` + its solve call ~:885-901, :1383-1391),
`src/engine/l5/registry.rs` (`run_detectors_cross_app` uses `cross_app_inputs()`),
`src/engine/l4/summary_runner.rs` (role write-back ~:738-774 skipped for `fixed_roles`). Rows: 3a, 3b, 3c.

**Interfaces:** `R3a5CrossAppBase.fixed_roles: HashSet<RoutineId>`; `pub(crate) fn cross_app_inputs(&self) -> CrossAppInputs<'_>`
(the ONE constructor; registry, events and policy use it); `CrossAppInputs.fixed_roles`; the solver entry
gains a `fixed_roles: &HashSet<RoutineId>` parameter (empty set = today's behaviour for single-app).

- [ ] Run 3a/3b/3c; confirm red.
- [ ] Leaf predicate: leaf iff `body_available` dependency routine with `graph.edges_by_from.get(&id).map_or(true, |e| e.is_empty())`.
- [ ] `fixed_roles` = dependency routines with an out-edge; thread it to every cross-app solve; the solver
      keeps initial unknown roles for its members.
- [ ] Factor `cross_app_inputs()`; `run_detectors_cross_app` uses it (no drift).
- [ ] Tests green; `--test r3`, `--test r4`, `--test cli`, `--lib` green; any golden move triaged.
- [ ] Proofs (3a, 3b, 3c a+b); `check-diff`; commit `fix: dependency event publishers reach the cross-app store with conservative roles (#70)`.

### Task 3: `events fanout` / `events chains` cross-app by default

**Files:** `src/engine/gate/events.rs`, `src/bin/alsem.rs`. Rows: 1a, 1b, 1d, 1e, 1f (events part).

**Interfaces:** `EventsFanoutOptions.single_app`, `EventsChainsOptions.single_app`; top-level `dependencies`
right after `kind` in both JSON formatters; human first line from `LoadedDependencies::human_line()`.

- [ ] Run the rows' tests; confirm red.
- [ ] `--single-app` clap arms (digest's doc text); cross-app build via `build_cross_app_base` + 
      `build_detector_context_with(&base.resolved, substrate::ALL, Some(base.cross_app_inputs()))`;
      `compute_analyzer_diagnostics_cross_app` for stderr; error naming `--single-app` when the base is `None`.
- [ ] Regen `tests/cli-c-events-goldens` ONLY after golden-diff-triager confirms every line is the
      `dependencies` key or the platform `dependencies:` human line.
- [ ] Tests green; proofs; `check-diff`; commit `feat: events fanout and chains load dependencies by default (#70)`.

### Task 4: `policy check` cross-app by default; CHANGELOG; CDO measurement

**Files:** `src/engine/gate/policy/pipeline.rs`, `src/engine/gate/policy/format_policy.rs`,
`src/engine/gate/policy/policy_engine.rs` (routine population filtered to the primary app before
evaluation), `src/bin/alsem.rs`, `CHANGELOG.md`. Rows: 1c, 1d, 1e, 1f (policy part), 1g.

- [ ] Run the rows' tests; confirm red.
- [ ] `--single-app`; cross-app build with `substrate::ALL | substrate::RAW_INHERITED_FACTS` and
      `cross_app_inputs()`; primary-only routine population; `dependencies` key; human line.
- [ ] Regen `tests/cli-c-policy-goldens` after triage (same rule as Task 3).
- [ ] CDO before/after per the spec's Measurement plan (`master` binaries vs branch, release-fast):
      events/policy time and output deltas, analyze findings by detector (leaf predicate), prove /
      fingerprint / diff byte-identical, `--program-call-graph-stats` byte-identical. Triage every new
      policy finding and every moved analyze finding.
- [ ] CHANGELOG `## [Unreleased]`: Added (events/policy cross-app, `--single-app`, `dependencies`),
      Changed (prove/fingerprint/diff stay single-app — the spec's reason text), Fixed (fingerprint closure,
      git boundary, dependency publisher store rows), with the CDO numbers.
- [ ] Tests green; proofs; `check-diff`; commit `feat: policy check loads dependencies by default; CHANGELOG (#70)`.
