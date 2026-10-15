# Issue #70 — cross-app follow-ups from #66 (design spec)

Evidence for every claim below is in `2026-10-15-issue-70-cross-app-follow-ups-probes.md` (same
folder), cited as [A3], [D2], etc. (section letter + row number). The probe fixture and outputs it
cites (`scratchpad/70/…`) are the conductor's scratch files; the tables quote the results.

## Goal

1. Each of `prove`, `fingerprint`, `diff`, `events` (fanout, chains) and `policy check` either
   loads dependencies by default through `build_cross_app_base` (with `--single-app`, a
   dependencies statement and a no-dependency contract test), or has a written reason in the
   CHANGELOG why it stays single-app.
2. `workspaceFingerprint` covers the whole dependency closure the loader reads, so swapping any
   loaded `.app` changes it, and nothing else does.
3. A fixture where a dependency routine's fixed-leaf summary differs from its solved summary
   discriminates the cross-app L4 store (`leaf_summaries`, `cross_app_db_effect_bundle`) in
   both `query` and `digest`.

## Non-goals

- Making the snapshot-based commands (prove, fingerprint, diff) cross-app. They need a
  cross-app snapshot composition that follows dependency-internal calls; it does not exist
  (see Design §1). Filed as a follow-up, not built here.
- Fixing digest's false `coverage: complete` on dependency call chains [C1-C2]. It is a #66
  defect found by these probes; file it as a discovery (it blocks any future cross-app prove).
- Adding `.al` sources to `workspaceFingerprint` [D3] (al-sem's input model; separate question).
- Changing `analyze`'s or `graph`'s mode.

## Acceptance matrix

| # | Acceptance bullet | Proof (test file :: test name) | Mutation that must make it fail |
|---|---|---|---|
| 1a | `events fanout` loads deps by default | `tests/cli/cli_events_policy_dependencies.rs :: events_fanout_reports_a_dependency_event_with_a_workspace_subscriber` — on the `RaiseDep`-style fixture, default `--scope primary` JSON has an entry for the dependency's `OnDepEvent` with `directSubscriberCount >= 1` and `dependencies.mode == "crossApp"`, `apps == [(DEP_GUID, "embedded")]`; `--single-app` has no such entry and `mode == "singleApp"`, `apps == []` | build the single-app model in the default branch of `run_events_fanout` → no entry (today's output: `totalEvents: 0` [A10]) |
| 1b | `events chains` likewise | same file :: `events_chains_walks_a_dependency_event_with_a_workspace_subscriber` | same mutation in `run_events_chains` |
| 1c | `policy check` loads deps by default | same file :: `policy_check_resolves_a_dependency_table_name` — a `--policy` YAML with one rule `capability.op: insert` + `capability.resource.table.name: "Customer"` (`onUnknown: fail-open`) on a workspace routine inserting the dependency's Customer: default → 1 finding, `--single-app` → 0 (the table is unresolved there) | single-app model in the default branch → 0 findings. Second mutation: cross-app model but the single-app detector context (`build_detector_context` without `CrossAppInputs`) → must be caught by `policy_check_follows_dependency_calls` (a subscriber calling `Outer`→`Inner` that commits; default `no-commit-in-event-subscribers` finds it only when `injected_typed_edges` reach the cone). Verify both predictions red-first. Third (R1-08, R2-03): `policy_check_matches_an_inherited_dependency_fact` — a rule with `facts: inherited` and `capability.op: insert` (the supported fields, `predicate_fields.rs:177-281`) on a workspace routine that reaches the dependency insert only through `Outer`/`Inner` → exit 0 and one finding cross-app. Mutation: cross-app demand without `RAW_INHERITED_FACTS` → the run panics / exits non-zero (`full_summary.rs:107-116`) → fails. Fourth (R1-03, R2-02): `routinesEvaluated` equals the HAND-STATED number of workspace routines in the fixture (dependency routines excluded) in both modes; the per-mode matched / skipped / `findingsEmitted` are asserted against hand-stated expected values for each mode separately (they legitimately differ, because the dependency table resolves only cross-app); no finding is anchored in a `dep:` file → evaluating dependency routines (filtering only findings) makes `routinesEvaluated` exceed the stated count, fails |
| 1d | `--single-app` flag exists on the three subcommands | the 1a–1c tests spawn `CARGO_BIN_EXE_alsem` with `--single-app` | drop the clap arm → clap rejects the flag, test fails |
| 1e | dependencies statement | 1a–1c assert `dependencies` in JSON; human output starts with `dependencies: XBase (embedded)` only when apps loaded | omit the key / always print the line |
| 1f | no-dependency contract (output equals single-app) | same file :: `without_dependencies_events_and_policy_cross_app_equal_single_app` — every argv of `cli_c_events_differential` + `cli_c_policy_differential` (all formats: json, human, sarif), cross vs `--single-app`: exit codes equal, **stderr equal** (analyzer diagnostics); the DEPENDENCIES STATEMENT is asserted per mode, independently (R1-04): single-app → `{mode: singleApp, apps: []}` and no human line; cross-app → `mode: crossApp`, `apps` = exactly the `source: "missing"` rows for System and System Application when the fixture's `app.json` declares `platform` (else `[]`), and the human first line is present iff `apps` is non-empty; EVERYTHING ELSE (JSON minus `dependencies`, human minus that first line, SARIF) byte-equal across modes. Plus `without_any_declared_dependency_no_statement_line`: a fixture with no `platform`/`dependencies` → `apps: []` and no human line in either mode. Floor on comparisons | print the human `dependencies:` line with zero apps → the no-declaration test fails (the #66 discrimination); omit the missing-row append loop (`full.rs:2160-2167`) → cross-app `apps` loses its System / System Application rows → the per-mode statement assertion fails (R3-02; removing the `found.is_none()` filter cannot fail here, since every ledger row of these fixtures is missing — that filter is covered by 1a/1b, whose fixture has a FOUND embedded dependency and asserts `apps` = exactly `[XBase (embedded)]` plus the platform rows if declared); the single-app detector runner for stderr in cross-app mode is NOT caught (no dependency) — stated limit |
| 1g | prove / fingerprint / diff reason in CHANGELOG | CHANGELOG `### Changed` (or `Notes`) entry, text in Design §1 | review item (no test) |
| 2a | fingerprint changes when the closure changes | `tests/cli/cli_workspace_fingerprint_closure.rs :: fingerprint_tracks_an_ancestor_dependency_swap` — tempdir with a `.git` dir at the root, the `.app` in the PARENT `.alpackages`; `digest … --format json` and `fingerprint --format json` (full snapshot) before/after rewriting the `.app` bytes → both `workspaceFingerprint`s differ; precondition asserted: the cross-app digest lists the app (`apps == [embedded]`) | revert `derive_inputs` to `read_dir(<ws>/.alpackages)` → fingerprint unchanged (today's behaviour [D2]) |
| 2b | own-folder swap (precondition, already true today) | same file :: `fingerprint_tracks_an_own_dependency_swap` | not a discrimination of the change (passes on master [D2]); kept as the regression pin |
| 2c | does not change otherwise | same file :: `fingerprint_ignores_everything_but_the_closure` — same closure twice → equal; add a non-`.app` file to the ancestor `.alpackages` → equal; an ancestor ABOVE the `.git` boundary gets an `.app` → equal (nested workspace; the repository-root case is 2g) | hash every file in the cache folder / ignore the `.git` boundary → differ |
| 2d | no golden moves | `scripts/check-goldens` green with zero diff | — (measurement: 171 files pin values [D7], none has a closure input [D8]) |
| 2f | a readable swap is seen while another package is unreadable (R1-05) | same file :: `fingerprint_tracks_a_swap_beside_an_unreadable_package` — own `.alpackages` with a readable `a.app` and a DIRECTORY named `broken.app` (discovered as a package, fails `fs::read` → a real `read-failed` error; `.alpackages` that is a file would be skipped by `is_dir()` before any read, `dependencies.rs:365-367`). Precondition asserted first: the fingerprint's inputs contain a `dep-package-unreadable` input keyed `.alpackages/broken.app`. Then rewriting `a.app` changes the fingerprint, and the same tree without `broken.app` fingerprints differently | `dependency_closure_entries` returns early on the first error (today's `?`) → `a.app`'s entry vanishes and the swap is not seen |
| 2h | an unreadable package fingerprints the same wherever the tree lives (R2-04) | same file :: `unreadable_package_fingerprint_is_location_independent` — the 2f tree built in two different temp directories → equal fingerprints | hash the error message instead of the category → the absolute path differs → fails |
| 2g | the `.git` boundary holds at the workspace itself (R1-06) | `dependencies::tests::own_git_root_stops_the_ascent` (unit) — a workspace whose own folder has `.git` (once as a directory, once as a file containing `gitdir: …`) and an `.alpackages` above it → `find_all_alpackages_folders` returns only the own cache; a nested workspace (`.git` one level up) still reads that level's cache; and `cli_workspace_fingerprint_closure.rs :: fingerprint_ignores_a_cache_above_a_repository_root_workspace` → the fingerprint does not change when the above-root `.app` changes | delete the new `project_root/.git` check → the above-root cache is returned / the fingerprint changes |
| 2e | one discovery for loader, closure digest and fingerprint | unit test `snapshot_full::tests::derive_inputs_uses_the_closure_entries` over a hand-built tree with own + ancestor caches: the `dep-package` input paths equal the closure keys | a second private walk that drifts (e.g. drops ancestors) |
| 3a | the leaf input is discriminated (query) | `tests/cli/cli_digest_query_dependencies.rs :: dependency_publisher_dispatch_reaches_the_store` — `RaiseDep` fixture: cross-app `query effects --routine RaiseDep` lists `DB_INSERT Customer` (origin `HandleDep`, `dep:` anchor) and `DB_INSERT "Ws Flag"` (origin `HandleInWs`), `tempState known(false)` | make dependency publishers fixed leaves again (today's predicate) → `effects: []` [E6]. See Design §3 for why "empty `leaf_summaries`" is no longer the mutation |
| 3c | a solved dependency publisher keeps unknown parameter roles (R2-01) | `src/engine/l4/capability_cone.rs` (or the r3 umbrella) :: `dependency_publisher_roles_stay_unknown` — a dependency publisher `OnDepRec(var Rec: Record Customer)` whose dependency subscriber modifies `Rec`; checked through the production paths that expose each fact (R3-01, R4-01): DB ROWS through the store from `cross_app_db_effect_bundle` (it returns rows only, `effect_query_cli.rs:205-217`, `effect_store.rs:709-716`; the bundle API is not widened); PARAMETER ROLES through (ii) the detector context `build_detector_context_with(&base.resolved, demanded, Some(base.cross_app_inputs()))` (`ctx.parameter_roles_by_routine`), the path analyze uses, and (iii) `project_r3a5_cross_app` (`capability_cone.rs:3552-3567`, roles and effects). Expected, hand-stated: the publisher `OnDepRec` has its subscriber's DB row, and its five path-sensitive role fields are all unknown; a workspace forwarder `Fwd(var Rec: Record Customer)` whose body is ONLY `Lib.OnDepRec(Rec)` has `requires_loaded_at_entry`, `mutates_before_load` and `required_loaded_fields_at_entry` = unknown, while its exit fields keep the walker's existing behaviour (`dirtyAtExit: no`, `currentLoadedFieldsAtExit: full`, `cfg_walker.rs:1340-1368`), which this issue does not change | (a) let the solver write back roles for `fixed_roles` members → concrete roles → fails; (b) pass an EMPTY `fixed_roles` only at the detector-context call → path (ii) fails while (iii) still passes, which proves path (ii) is checked on its own |
| 3b | digest's cross-app store is discriminated | `tests/cli/cli_digest_temp_state.rs :: cross_app_digest_reads_the_cross_app_store` — on the same fixture (R1-01): the `RaiseDep` digest entry has EXACTLY the two dispatch inserts (`DB_INSERT Customer` from `HandleDep`, `DB_INSERT "Ws Flag"` from `HandleInWs`), each with `detail.tempState` = `known(false)` (absent today [E6]), no `temp-state-incoherent` diagnostic, and their parts equal `query effects RaiseDep`'s; `--single-app` digest has neither insert (precondition that the dependency matters). Update the file header's stated limit | the discriminating break is reinstating publisher leaves (the defect) → `tempState` absent and query empty → fails. **Stated relaxation, for the panel to approve:** "digest uses the single-app store in cross-app mode" can no longer be told apart, because digest holds no single-app model in cross-app mode, and `build_db_effect_bundle(base.resolved)` over the base's edges equals the cross-app store once publishers are solved. The test pins the property the issue cares about (digest and query agree on the dependency dispatch, with temp state), not which builder produced it. Record the break red/green |

## Design

### 1. Per command

| Command | Decision | Reason (probe) |
|---|---|---|
| `events fanout` / `chains` | **cross-app by default**, `--single-app` | Single-app reports 0 events for a workspace subscriber to a dependency event [A10]; that is BC's main events case. `--scope primary|all` and `primary_routines` are already keyed on `dep_routine_ids` [A9]. Same doctrine as d43/d44/d45 (measure event detectors cross-app). |
| `policy check` | **cross-app by default**, `--single-app` | Rules match on table names and inherited facts [A11]; a dependency table has no name single-app, and the cross-app detector context's cone follows dependency calls via `injected_typed_edges` [A12]. |
| `prove` | **stays single-app** | Prove makes ABSENCE claims. Built like digest cross-app, `prove CallChain may-commit` would answer `no` while the dependency chain commits: digest cross-app reports `coverage complete`, no effects, no unresolved call on that routine [C1-C3]. Single-app answers `unknown` (blocked by `unresolved-callsite`). It can switch once the cross-app snapshot follows dependency calls or marks them partial (the discovery). |
| `fingerprint` | **stays single-app** | Its snapshot is composed over `build_r3a3_source_only_base`, a single-app cone [A4]; a cross-app snapshot over `base.resolved` would carry the same false `complete` coverage into `rootClassifications`/`coverage` [C2], and would add dependency routines to `identities`/`--inventory-only` (al-perf consumes it [A5]). Needs a cross-app snapshot composition first. |
| `diff` | **stays single-app** | It compares persisted fingerprint snapshots with workspace recompositions [A7]; it must use fingerprint's mode, and no snapshot records its mode, so switching one without the other makes old-vs-new diffs report mode drift as change. Moves with fingerprint. |

CHANGELOG reason text (one entry, `### Changed`): "`prove`, `fingerprint` and `diff` stay
single-app. Their output is a capability snapshot, and the cross-app snapshot does not yet follow
calls made inside dependency code: on a workspace routine that calls a dependency routine which
calls another that commits, it reports complete coverage and no commit. `prove may-commit` would
then answer `no`. `diff` compares fingerprint snapshots, so it keeps fingerprint's mode."

Implementation (events, policy), copying #66:
- `EventsFanoutOptions` / `EventsChainsOptions` / `PolicyCheckOptions` gain `single_app: bool`;
  `alsem.rs` adds the `--single-app` clap arm (same doc text as digest).
- Build: `single_app` → today's path; else `build_cross_app_base(ws, &model_id)` (error text
  like #66: "… the cross-app model could not be built (--single-app analyses the workspace
  alone)"), then `build_detector_context_with(&base.resolved, demanded, Some(CrossAppInputs{…}))`
  with the same fields `run_detectors_cross_app` passes. Factor that `CrossAppInputs`
  construction into one `R3a5CrossAppBase::cross_app_inputs(&self)` so registry, events and
  policy cannot drift (three callers → one helper; no other abstraction). `demanded` stays the
  caller's, exactly as today in each mode (R1-08): `events` passes `substrate::ALL`
  (`events.rs:364/495`), `policy check` passes `substrate::ALL | substrate::RAW_INHERITED_FACTS`
  (`pipeline.rs:251-266`); dropping `RAW_INHERITED_FACTS` in cross-app mode would silently break
  inherited-fact matching, and test 1c's inherited case (below) catches it.
- Analyzer diagnostics on stderr: `compute_analyzer_diagnostics_cross_app(ws, &base)` in
  cross-app mode (`run.rs:1073`), formatted as `analyzer_diagnostic_lines` does.
- Policy: the evaluated ROUTINE population is filtered to the primary app BEFORE evaluation
  (R1-03): `classify_roots` and `policy_engine` see only workspace routines (`primary_app`, as
  `is_primary_routine`), while the model keeps every dependency table, object and inherited fact
  for lookup. So `routinesEvaluated`, rule matched/skipped counts and `findingsEmitted` count
  workspace routines only. Test 1c asserts `routinesEvaluated` equals the hand-stated number of
  workspace routines in both modes, asserts matched / skipped / `findingsEmitted` against
  hand-stated per-mode values (they legitimately differ, because facts are mode-dependent,
  `policy_engine.rs:297-310`), and asserts that no finding is anchored in a `dep:` file (R2-02).
- Dependencies statement: events/policy JSON have no `payload` and no `schemaVersion` [A13], so
  add a top-level `dependencies` (`LoadedDependencies::to_json()`) right after `kind`, at the
  same relative position in `format_fanout_json`, `format_chains_json` and `format_policy_json`
  (R1-10). Human: the `human_line()` first line. SARIF: unchanged (no consumer reads it; say so
  in CHANGELOG). **`apps` is not always empty on the golden corpus (R1-04):** a fixture whose
  `app.json` declares `platform` has an implicit System dependency; with no package present,
  `loaded_apps` emits `source: "missing"` rows (`full.rs:2160-2167`) for BOTH implicit platform
  dependencies, System and System Application (`dependencies.rs:258-261`), and `human_line()`
  prints `dependencies: System (missing), System Application (missing)` (in `loaded_apps`' sorted
  order). In `--single-app` mode the statement is `LoadedDependencies::default()` (`apps: []`,
  no human line), as #66's digest does (`digest_cli.rs:1571-1579`). 24 r0-corpus fixtures and
  the vendored `cli-c-policy` fixtures declare `platform`. This is the statement working as
  designed (#66 digest behaves the same), not a defect.
- `events` `--scope all` in cross-app mode now includes dependency-only events; document it.

### 2. `workspaceFingerprint`

Finding [D2]: the fingerprint ALREADY hashes `<ws>/.alpackages/*.app`; it misses only ancestor
caches the loader also reads. Minimal correct change: make `derive_inputs` read its
`dep-package` inputs from the closure discovery, not its own `read_dir`.

- Extract from `semantic_golden::dependency_closure_digest` a
  `pub fn dependency_closure_entries(root) -> ClosureEntries { entries: Vec<(String /*key*/, PathBuf)>, errors: Vec<(String /*key*/, String /*message*/)> }`
  (R1-05): the keyed, sorted list it already builds, PLUS every listing or read failure keyed by
  the same deterministic `../`-relative key (a cache folder key `../.alpackages/`, or a file key).
  Readable entries are never dropped because another cache failed. The closure digest keeps its
  current behaviour (it returns `Err` when `errors` is non-empty, as today), so its stamped
  values do not change. `derive_inputs` emits one `dep-package` input per entry with
  `path = key`, `contentHash = sha256(bytes)`, and one `dep-package-unreadable` input per error
  with `path = key`, `contentHash = sha256(<category>)`, where the category is the stable marker
  `listing-failed` or `read-failed` (R2-04). The human-readable message (which contains machine
  paths and OS wording, `semantic_golden.rs:246-247, 273-275`) is never part of the fingerprint.
  Test 2h relocates the same tree with the same failure and asserts an equal fingerprint. So an unreadable cache never fingerprints
  like a readable one, and a readable package swap still changes the fingerprint while another
  cache is unreadable (test 2f).
- For the workspace's own cache the key is `.alpackages/<name>`, byte-equal to today's
  `rel()` path, so every fingerprint without an ancestor cache is unchanged: **zero golden
  moves** [D7-D8]. With the other fingerprint inputs (`app.json`, roots configuration, coverage
  policy, alsem version) held constant, the fingerprint changes iff a closure entry is added,
  removed, renamed or changes bytes, which is the same sensitivity as the closure-v2 digest
  (R1-07). It does not embed the literal `closure-v2:` string; that is a deliberate deviation
  from the issue's wording, for the panel to approve (below). Zero golden churn is a consequence,
  not the proof: tests 2a-2f are the proof.
- **Git boundary (R1-06).** `dependencies::find_all_alpackages_folders` (`dependencies.rs:351-378`)
  adds the workspace's own cache and then ascends from `project_root.parent()` WITHOUT first
  checking `project_root/.git`, so a workspace that is itself a repository root still reads
  caches above it. That contradicts the stated "stops at the first directory with `.git`"
  contract of the loader, the closure digest and this fingerprint. Fix it once in that helper:
  if `project_root.join(".git").exists()`, do not ascend. This changes the LOADER for a
  repository-root workspace that has an `.alpackages` folder above it (a pre-existing bug; CDO is
  unaffected, since `Cloud/` has no `.git` [D9]). CHANGELOG `### Fixed`. Tests 2c and 2g cover a
  repository-root and a nested workspace, with `.git` as a directory and as a worktree file.
- Why not append the literal `closure-v2:` string as an extra input (the issue's wording):
  redundant with the `dep-package` rows, and it moves all 171 golden files (plus every
  snapshot `inputs` array) for no additional sensitivity. Panel call; the spec recommends the
  shared-entries form.
- Mode-independent: `--single-app` runs report the same fingerprint (the mode is stated in
  `payload.dependencies`).
- Failure shape: covered by `errors` above. `derive_inputs` stays infallible. Today it skips an
  unreadable cache silently.

### 3. `leaf_summaries` and the fixture

Finding [E3-E7]: dependency call edges are already cut from the solver graph, so a fixed leaf
differs from a solved routine ONLY at a dependency event publisher that has subscribers. There,
the leaf drops the subscribers' writes from the L4 store while digest's cone keeps them: on
`RaiseDep`, digest reports two `DB_INSERT`s without `tempState`, `query effects` reports none.
So the fixture the issue asks for exists (a dependency PUBLISHER, not a subscriber), but on it
the current leaf behaviour is the bug. A test that fails "when `leaf_summaries` is emptied" would
pin that bug.

Design: a dependency routine is a fixed leaf only if it has no out-edge in the combined graph
(`graph.edges_by_from.get(&id).map_or(true, |e| e.is_empty())`, so an empty edge list counts as
none; R1-11). One predicate in `build_cross_app_base`
(`capability_cone.rs:3410-3416`), shared by analyze, digest, query and r3a5. Then the store
follows a dependency publisher's dispatch exactly as the cone does; the "own direct effects
only" contract still holds for every dependency CALL (cut edges). Update the kept-contract doc
at `capability_cone.rs:3293-3296`.

What else the predicate changes (R1-02). A fixed leaf is not only "no out-edge rows". It also
keeps UNKNOWN path-sensitive parameter roles (`summary_runner.rs:546-568`), which a solved
routine replaces through the control-flow walker (`:738-774`, `cfg_walker.rs:85-97`). Role
composition walks source callsites and the routine's body; it does NOT compose the synthetic
subscriber dispatches (`combined_graph.rs:332-340` creates dispatch edges with no callsite). So a
solved publisher whose subscriber reads or dirties a forwarded record would report falsely
concrete roles (no entry requirement, `dirtyAtExit: no`) (R2-01). Therefore: **a dependency
publisher is solved for its effect ROWS only; its parameter roles stay the leaf's values for
the whole solve** (the five path-sensitive fields unknown; the body-local fields such as
`persistsCurrentRecord`, `writesFields` and `validatesParam` from the publisher's own body,
exactly as for every dependency leaf). Mechanism: `build_cross_app_base` computes
`fixed_roles: HashSet<RoutineId>` (the dependency routines that have an out-edge) and stores it
on `R3a5CrossAppBase` next to `leaf_summaries`. It travels through EVERY cross-app solve (R3-01):
a new `CrossAppInputs.fixed_roles` field, filled by the shared `R3a5CrossAppBase::cross_app_inputs()`
constructor (so the detector context used by analyze, events, policy and digest diagnostics
gets it, `detector_context.rs:885-892, 1383-1391`), and the base's own solve in
`cross_app_db_effect_bundle` / `project_r3a5_cross_app` (`capability_cone.rs:3430-3453,
3486-3493`). For a routine in the set the solver keeps the initial role facts (path-sensitive
fields unknown, body-local fields from its body) and never replaces them
(`summary_runner.rs:738-774` skips the role write-back for it), so callers composing through it
see the leaf's roles, as today. Test 3c (below) pins both paths. The same gap exists for WORKSPACE
publishers today (pre-existing, not introduced here): filed as a discovery, not fixed in #70. The
change is still measured on CDO (analyze findings by detector, before and after, every moved
finding triaged).

Consequence for the Acceptance (a restatement for the panel to approve). After this change,
emptying `leaf_summaries` is a no-op FOR DB-EFFECT ROWS (every remaining leaf has no out-edge).
It is NOT a no-op for full summaries (parameter roles), so `leaf_summaries` is KEPT, and #70
does not consider deleting it. The issue's mutation ("emptying `leaf_summaries` fails a test") is
restated as "making dependency publishers fixed leaves again fails a test", because that is the
only place the leaf input changes a DB row.

Fixture (in `tests/cli/cli_digest_query_dependencies.rs`, reused by `cli_digest_temp_state.rs`
through a shared helper in `tests/common/`): the probe fixture's dependency (`Dep Lib` with a
global `[IntegrationEvent] OnDepEvent`, `Dep Sub.HandleDep` inserting Customer, plus
`Outer`/`Inner`) and workspace (`RaiseDep`, `Ws Sub.HandleInWs` inserting `Ws Flag`, `CallChain`).
Hand-stated precondition: the dependency is `embedded` and `RaiseDep`'s digest entry has the two
dispatch inserts (already true today [E6]).

## False-positive and failure shapes

- Events cross-app on CDO will list many Base Application events with Continia subscribers that
  were invisible: these are true events, not noise; `--scope primary` keeps only events touching
  a workspace routine.
- Policy cross-app may add findings (dependency table names now match `except` clauses or rule
  selectors; inherited facts reach into dependency code). Each new finding on CDO must be
  triaged against source before merge; a rule firing on a dependency-anchored root is a bug
  (roots must be workspace routines).
- Missing primary app (`app.json` absent) → `build_cross_app_base` returns `None` → the command
  errors and names `--single-app`, as #66 does. Check every events/policy golden fixture has an
  `app.json` (all checked ones do [A14]).
- The leaf change can ADD rows to analyze's L4 summaries where a workspace routine calls a
  dependency publisher directly → possible new analyze findings. Measure (below); no golden
  fixture is expected to have the shape.
- Fingerprint: symlinked caches dedup by canonical path (same as the loader); a cache above the
  `.git` boundary is ignored (same as the loader, after the R1-06 fix).
- The git-boundary fix changes which packages the LOADER reads for a repository-root workspace
  with an `.alpackages` above it. Those packages were never part of the project per the stated
  contract. No golden or CDO input has that layout [D8-D9].

## Measurement plan

CDO = `U:/Git/DO-cdo-baseline/Cloud` (pinned, detached `bc3ccb18`), `release-fast`, warm, best
of 3, wall time + peak working set; before = master `b77025e4`.

| What | Before | After | Expect |
|---|---|---|---|
| `events fanout` (json, primary / all) | time, MB, `totalEvents`, entries | default + `--single-app` | single-app byte-equal to before; cross-app ~base cost (≈18.5 s + context, ~2.8 GB like #66), more entries |
| `events chains` (json, primary / all) | same | same | same |
| `policy check` (json, default policy) | time, MB, findings by rule | default + `--single-app` | single-app byte-equal; cross-app findings delta, every new one triaged |
| `prove`, `fingerprint --format json`, `diff ws ws` | output sha | output sha | byte-identical (they stay single-app; fingerprint unchanged because CDO's closure is its own folder [D9]) |
| `digest --file CDOIssueDocument.Codeunit.al` | json sha, time | after leaf change | equal unless CDO has a workspace→dependency-publisher call; report the count of such call edges (the population) |
| `alsem analyze` (default, cross-app) | findings count by detector, time | after leaf change | equal unless that population is non-zero |
| `aldump --program-call-graph-stats` | histogram sha | | byte-identical (resolution untouched) |
| `scripts/cdo-gate` | | green | |

Golden families expected to move:
- `--test cli`: `tests/cli-c-events-goldens/*.json` (33) and `tests/cli-c-policy-goldens/*.json`
  (12) gain only the top-level `dependencies` block (`mode: crossApp`; `apps` is `[]`, or a
  `System` and a `System Application` row, both `source: missing`, for a `platform`-declaring
  fixture). The `.human.txt` goldens of `platform`-declaring fixtures gain exactly one first
  line, `dependencies: System (missing), System Application (missing)` (R1-04); every other `.txt` and every `.sarif` is unchanged. `manifest.json` files are not
  outputs and are excluded from move counts. Verify with the golden-diff triager that nothing
  else moved; the exact per-file list is produced by the regen, not predicted.
- `cli-b-*` (fingerprint/snapshot/digest/prove/diff): none (fingerprint change is golden-stable).
- `--test r3` / `--test r4` cross-app, `tests/gate-goldens` (analyze): none expected from the leaf
  predicate (no fixture has a workspace→dependency-publisher call); a move there must be
  triaged as a real dispatch row, never blessed blind.
