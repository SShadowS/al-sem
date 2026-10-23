# Issue #157: apps that depend on sibling apps as source

Status: spec. Base `70fc2e9b`. Probes and their outputs: the ledger
`.agent/issue-157/ledger.md` (section "Assumption probes") and `/work/bench/157/out/`.
Deadline: release v2.0.1 by 2026-10-24 (latest 10-25), for CentralGauge on 10-27.

Terms used below:
- **primary app**: the app folder a command is pointed at. Findings, digest entries and diff
  subjects are reported for it.
- **source dependency**: another app folder, given on the command line, whose `.al` files stand
  in for a dependency that has no `.app` file.
- **sibling**: another app folder in the same repository tree (CentralGauge's refapp has 7).
- **cross-app model**: the model `analyze` and `digest` build by default: the primary plus the
  dependency code it calls (engine-switch S8). `diff` and `fingerprint` are single-app
  (`src/engine/gate/diff/cli.rs:122,139`, `src/engine/l5/fingerprint_cli.rs:386`).

## Goal

Make the four commands CentralGauge runs correct on a tree of apps that depend on each other
as source, with each object carrying its real app id and `internal` boundaries kept:
`alsem analyze --format json`, `al-call-hierarchy --project <ws> --analyze --format json`,
`alsem diff <start> <final>`, `alsem digest --diff`. Retire the synthetic one-app merge in
`analyze-campaign.ts`.

## Non-goals

- Several primary apps in one run (sized below: at least 5 working days).
- A root folder with several `app.json` files as input, or `.code-workspace` input. The
  existing refusal stays (`src/engine/snapshot.rs:359-369`).
- Making `diff` or `fingerprint` cross-app. That is #160 step 9.
- Seeing event subscribers that live in an app which depends on the primary (K9, below).
- The LSP server. Its dependency tier keys a unit on the `.app` path and stamp
  (`src/program/dep_cache.rs:278-292`); a source unit has neither, so it would not see a
  sibling edit. Not reachable: the LSP gets no `--source-dependency`.
- `--source-dependency` on `graph`, `query`, `events`, `policy`, `prove`.
- Fixing the pre-existing d14 false positive on an `internal` routine of an app that has
  `internalsVisibleTo` (`internal_reachable_externally` is hard-wired `false`,
  `src/engine/l5/detector_context.rs:1484`). Filed as a discovery.

## The key question: one primary per run

**Can a sibling edit go unseen?** Measured on the 3-app fixture plus scratch variants
(ledger rows K1-K9):

| Command | Run only the EDITED apps as primary | Run EVERY app as primary (others as source deps) | Today's synthetic merge |
|---|---|---|---|
| `analyze` | **misses** findings a sibling edit causes in a caller app. K3: Core's `Round2` gains `Commit()`; Rental's loop over it gets `d34-commit-in-loop`, only in Rental's run | sees it (K3) | sees it |
| `diff` | the edit shows in its own app (K2) | same; callers in other apps never show it, because `diff` is single-app (K1: Rental's snapshot is byte-identical) | shows it in every caller: 4 findings vs 1 (K5) |
| `digest --diff` | correct, if the patch is app-relative | unedited apps give exit 2 (no changed roots) | n/a |
| `al-call-hierarchy --analyze` | one run on the root already covers every app (P9) | same | same |

One more difference from the merge, in every per-app mode (K9): a publisher app does not
see a subscriber that lives in an app depending on it, because that app is outside the
publisher's dependency closure. In K9 the merge reports `d34-commit-in-loop` on Core's loop
(the commit is in Rental's subscriber); the per-app loop does not, and reports
`d12-dead-integration-event` instead. Core's run there is byte-for-byte what master gives
for Core analyzed alone. So the per-app loop gives exactly alsem's normal meaning of "this
app with its dependencies". The merge gives a whole-tree meaning that alsem does not offer
for any input today.

**Answer.**
1. `analyze` needs ALL apps as primaries, not only the edited ones. One top app with
   `--scope all` does not replace that (K4: 0 findings, the Rental d34 is lost).
2. "All apps in one run" adds no correctness over a loop: each run reports only its own
   primary (`--scope primary`, the default), the guids are disjoint, and passing every other
   sibling is harmless (K7: byte-identical output). So the merge step is concatenation.
3. Several primaries in one run is far bigger. 171 references to the single primary
   (`primary_app_ref`, `workspace_app`) in 21 files, 74 more primary/scope checks. Snapshot
   schema 4 requires exactly one primary (#220 spec, lines 159-160). Demand is seeded from
   the primary's callers. Estimate: at least 5 working days plus golden and CDO re-triage,
   and even then K9's subscriber shape needs a world-model change on top. Not deliverable
   by 2026-10-24.
4. Decision: one primary per run, and the harness loops over all apps. Cost (K8, release-fast,
   with Base Application's embedded source): `analyze` about 9-10 s per app, `diff` about
   7 s per app. For 7 apps: about 70 s plus 50 s per execution.
5. What CentralGauge's numbers lose against the merge: `diff` propagation into caller apps
   (until #160 step 9) and downstream event subscribers (K9). What they gain: real app ids,
   `internal` boundaries, and cross-app access findings.

**Harness rule for `analyze-campaign.ts`** (for each tree `T` with apps `A1..A7`):
- Put the locked Microsoft packages once in `T/.alpackages`. The loader reads every ancestor
  `.alpackages` up to the git boundary (`dependencies::discover_app_files`,
  `src/dependencies.rs:451`).
- `analyze`: for every app `A`: `alsem analyze T/A --source-dependency ../B …` (every other
  app `B`) `--format json --require-dependencies`. Concatenate `payload.findings`; sum the
  summary counts. Keep the default `--scope primary` (`--scope all` repeats dependency
  findings across runs).
- `diff`: for every app `A`: `alsem diff <start>/A <final>/A --source-dependency ../B …`.
  A relative path resolves against each side's own folder, so each side gets its own
  siblings. Unedited apps give 0 findings today; after #160 step 9 they report the
  propagated changes.
- `digest --diff`: for every app `A` with changed files: `git diff --relative=A/ > A.patch`,
  then `alsem digest --diff A.patch <ws>/A --source-dependency ../B …`. A tree-relative patch
  matches nothing and exits 2 (K6).
- `al-call-hierarchy --project T --analyze --format json`: once, on the root.

## Acceptance matrix

The fixture is `tests/fixtures/multi-app/{start,final}/{Core,Fleet,Rental}`. It carries no
`platform`/`application` fields, so no Microsoft package is needed and CI can run it.
`Core` has public `Round2` and `internal Secret` with `internalsVisibleTo` Fleet; `Fleet`
calls both; `Rental` calls `Fleet.DailyRate` and `Core.Round2`, and has an unused
`local procedure Unused()`. `final` changes a Rental body (adds `Commit()` to `Quote`) and a
Core body.

| # | Issue item | Status | Test or measurement |
|---|---|---|---|
| 1 | `analyze Rental --source-dependency Core --source-dependency Fleet --format json --require-dependencies` exits 0, no `DIAG-dependencies` row for CGR Core/Fleet; zero `unknown` for the Rental→Core/Fleet calls | **not deliverable as written**, because the issue's fixture has Rental call `internal Core.Secret()`. AL forbids that call; the engine reports it as `internalNotVisible`, an `unknown` edge, so the run exits 4 (P1). Replacement: Rental calls `Core.Round2` instead; the forbidden call moves to item 3's negative case | cli member `cli_source_dependency.rs`: exit 0 and no such rows. Measure: `aldump --program-call-graph-stats` with the flag shows `primaryScoped.unknown == 0` (probe: `resolvedSource 2, unknown 0`) |
| 2 | Identity: with `--scope all`, Core objects carry Core's guid, Rental objects Rental's; no third guid | deliverable | same member: every finding's `objectId` guid equals the guid of the folder named in `primaryLocation.file` (`dep:<guid>:…` or `ws:…` = Rental); Rental's guid appears (d14 on `Unused`), Core's appears (d14 on `Secret`, P3), Fleet's does not |
| 3 | `internal`: Fleet primary resolves `Core.Secret()`; Rental primary gets `internalNotVisible`; discrimination by dropping the friend fill-in | deliverable | lib test in `src/snapshot/snapshot.rs` tests (or `program_resolve_harness`): Fleet → `resolvedSource`, no unknown; Rental with an added `Secret()` call (written into a temp copy) → `unknownByReason.internalNotVisible == 1`. Discrimination: empty the source unit's `internals_visible_to` → the Fleet assertion fails (P4 measured `unknown 1` then) |
| 4 | `diff <start>/Rental <final>/Rental --source-dependency …` exits 0, loads both sides, a Rental body edit is a diff subject with Rental's guid | deliverable, with paths `../Core ../Fleet` (relative to each side) | golden `tests/cli-b-goldens/multi-app-diff.golden.json`: `capability-gained-commit` on `Quote`, stableId prefix `3333…0003`. The Core edit does not appear in Rental's diff (K1; single-app, a stated limit) |
| 5 | `digest --diff <patch> Rental --source-dependency …` names changed Rental routines with Rental's guid; "exit 0 or 2 only for findings" | **wording not deliverable as written**, because digest's exit 2 means "zero changed roots", not findings (`src/engine/l5/digest_cli.rs:1682`). Replacement: exit 0 with an app-relative patch; the entries name `Quote` with Rental's guid | golden `tests/cli-b-goldens/multi-app-digest.golden.json` |
| 6 | `al-call-hierarchy --project <fixture> --analyze --format json`: every row has `app_id` = its folder's `app.json` id; `file` root-relative | deliverable; the test is a cli umbrella member, not a lib test, because the code is in the binary (`src/main.rs:185-300`) | cli member runs `CARGO_BIN_EXE_al-call-hierarchy` on `tests/fixtures/multi-app/start`: rows for `Util.al`, `Vehicle.al`, `Rental.al` carry `Core/src/Util.al` etc. and the matching guid |
| 7 | Unreadable sibling `app.json`, empty `id`, same guid twice → exit 3 naming the folder | deliverable. "Same guid twice" covers two source dependencies, and a source dependency equal to the primary | three cli cases (temp copies), each asserts exit 3 and the folder path in stderr |
| 8 | No flag → every golden byte-identical; `scripts/ci-steps task` green | deliverable | `scripts/check-goldens`, `scripts/ci-steps task` |

## Design

1. **Input.** `InputNeeds` gains `source_dependencies: Vec<PathBuf>`. `InputSet::read` resolves
   each path (a relative one against the workspace folder), and reads its `app.json` and `.al`
   texts once, frozen like the workspace's own (the #160 "one disk reader" rule,
   `src/program/inputs.rs:182`). Stored as `InputSet.source_dependencies:
   Vec<SourceDependencyInput { dir, app_json: FileInput, app_json_value, al_files }>`.
2. **Validation** (one function, called by each command before work starts): the folder's
   `app.json` must read and parse, its `id` must be non-empty, it must hold no nested
   `app.json` (`count_app_json_paths`, `src/source_text.rs:145`), and no guid may repeat
   across the primary and the source dependencies. Any failure: exit 3 (`CONFIG_ERROR`) with a
   message naming the folder.
3. **Snapshot.** At the end of `SnapshotBuilder::build_inner` (`src/snapshot/snapshot.rs:415`),
   one `AppUnit` per source dependency: `source` from its `.al` files with tier
   `EmbeddedSource`, `abi: None`, `app_path`/`app_stamp: None`; `id`, `compilation`
   (`context_from_app_json`), `declared_deps` plus `append_implicit_ms_tier_deps`, and
   `internals_visible_to` from `workspace_friend_apps` on its own `app.json`. This is the
   probe's shape (P1-P4), already handled end to end (`build_dep_nodes` takes source units,
   `src/program/build.rs:215-220`; friends wired for every unit, `build.rs:519-559`).
4. **Stale `.app`.** A `.app` unit with the same guid as a source dependency is dropped, and
   the drop is reported on stderr (P6; same reason as the self-dependency guard,
   `snapshot.rs:326-333`).
5. **CLI.** `--source-dependency <dir>` (repeatable) on `analyze`, `digest`, `fingerprint`,
   `diff`. On `diff` it applies to each side that is a workspace folder, resolved against
   that side's folder. With no flag, nothing changes (an empty list).
6. **`al-call-hierarchy --analyze`.** `ProcedureMetrics` gains `app_id` (the `id` of the
   nearest `app.json` at or above the file, up to the project root; `null` when none), and
   `file` becomes the project-relative POSIX path (`AlFile.rel_posix`, already returned by
   `discover_al_files`). CHANGELOG: Changed (a consumer reading `file` as a base name sees
   paths now).
7. **No change** to `modelInstanceId` (P5: it hashes ids and paths, not content, and only
   prefixes internal ids), the resolver, demand, detectors, snapshot schema or fingerprints.
8. **Docs.** CHANGELOG (Added: the flag; Changed: item 6). `--help` text names the
   relative-path rule.

## False-positive and failure shapes

- **Forbidden `internal` call in a fixture** shows as `unknown`/exit 4. It is correct AL
  semantics, not a bug (P1). Tests must not use it as a "should resolve" case.
- **d14 on a sibling's `internal` routine under `--scope all`** (P3, K9): pre-existing, the
  detector ignores the declaring app's friend list. The harness keeps `--scope primary`.
- **`d12-dead-integration-event` on a publisher whose only subscribers are in a dependent
  sibling** (K9): alsem's normal per-app meaning. Listed for CentralGauge, not fixed here.
- **A sibling passed with the wrong relative base** (cwd instead of the workspace folder):
  the folder does not exist → exit 3 naming it. The `--help` text states the rule.
- **A source dependency whose own declared dependency is missing**: reported as today, as a
  `DIAG-dependencies` row for that dependency, and `--require-dependencies` exits 4.
- **A source-dependency folder holding nested `app.json` files**: its walk would take the
  nested app's files. Refused as part of validation (exit 3), same rule as the primary.
- **Duplicate units of one guid** (stale `.app` plus source): prevented by design item 4.

## Measurement plan

All on `release-fast`, outputs under `/work/bench/157/out/after/`.
1. The fixture in `/work/bench/157/fixture` with the real flag: re-run P1, P3, P4, P6 and
   K3, K7, K9; each must match its ledger row (P1 with Rental calling `Round2`: `unknown 0`,
   exit 0).
2. The no-flag path: `scripts/check-goldens` byte-identical; `scripts/ci-steps task` green.
3. CDO: not needed for the flag (the no-flag path builds no source unit). The branch gate
   still runs `scripts/cdo-gate`; its unknown count must be unchanged.
4. Timing: one `analyze` with 6 source dependencies vs none on the fixture plus MS packages;
   report both (K7 showed no difference; re-check it with the real flag).

## Pre-stated fallbacks

- **If** a source unit with `abi: None` breaks a path the probes did not reach (a panic in
  `abi_node_counts`, a missing node): guard that site with `unit.abi.is_none()`, as
  `full.rs:2143` already does. Do not switch to synthesizing a `.app`.
- **If** `--require-dependencies` still fails only on Microsoft packages: that is the
  caller's `.alpackages`, as today; the test fixture carries no MS dependency.
- **If** relative-to-workspace paths prove confusing on `analyze`/`digest`: keep the rule
  (one rule for all commands is what makes `diff` work); absolute paths behave the same.
- **If** CentralGauge needs `diff` propagation into caller apps for 10-27: keep the
  synthetic-merge run for `diff` only (its subjects carry the merged guid) until #160 step 9.
- **If** CentralGauge needs K9's downstream-subscriber findings: keep the merged `analyze`
  run beside the loop for that metric, and file a follow-up ("subscribers in dependent source
  apps") with K9 as its repro.
- **If** the loop's time (about 2 min per execution) is too slow: an in-process loop in alsem
  that shares one `DepCache` across primaries. Not in this issue.
- **If** task 3 slips past 10-24: ship tasks 1-2. `al-call-hierarchy` metrics then keep base
  names; CentralGauge disambiguates by object name, as today.
- **If** all three tasks cannot land by 10-25: ship task 1 plus the flag on `analyze` only,
  and keep the merge for `diff`/`digest`.

## Tasks

1. **Engine and validation.** `InputNeeds.source_dependencies`, `InputSet` reading
   (design 1), validation (design 2), the source units and the stale-`.app` drop (design 3-4).
   Lib tests that state their precondition literally: item 3 (both directions + the
   discrimination proof), the stale-`.app` drop (break: remove the drop, watch two Core units
   appear), the validation cases of item 7. No CLI change; goldens unchanged.
2. **CLI flag, fixture and cli tests.** `--source-dependency` on the four commands (design 5),
   `tests/fixtures/multi-app/{start,final}`, cli member `cli_source_dependency.rs` (items 1,
   2, 7 at the exit-code level), goldens for items 4 and 5, CHANGELOG, `--help`.
   Then `scripts/check-goldens --regen` (only the two new goldens may appear) and
   `scripts/ci-steps task`.
3. **`al-call-hierarchy --analyze` app id and relative file** (design 6, item 6), with its
   cli member and CHANGELOG line.

## Riskiest assumption

That CentralGauge accepts per-app semantics for its metrics: two shapes differ from the
merge they use today (`diff` propagation, K5; downstream subscribers, K9). Both are stated
above with a fallback; the engine work does not depend on the answer.
