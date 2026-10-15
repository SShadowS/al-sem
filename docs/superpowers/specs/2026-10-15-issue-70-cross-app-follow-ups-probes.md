# Issue #70 — assumption probes

Worktree `U:/Git/al-sem-issue-70-a1` at master `b77025e4`. Binary: `target/release-fast/alsem.exe`
(built for these probes, `cargo build --profile release-fast --bin alsem`, 2m21s).
Probe fixture: `scratchpad/70/tools/mkfx.py` writes `scratchpad/70/fx/{own,anc}/ws`
(an embedded-source dependency `XBase`, guid `bbbb7070-…`, as `.app` = 40 zero bytes + zip,
the format of `tests/common/symbol_app.rs`). `own` keeps the `.app` in `ws/.alpackages`,
`anc` in the PARENT `.alpackages` (a `.git` dir marks the boundary). Outputs in `scratchpad/70/out/`.

`spec.md` cites rows as section letter + 1-based row number within that section's table
(e.g. [A10] = the 10th row of section A, "`events` on a dependency event, today").

Fixture routines:
- dependency `Dep Lib`: `Outer()` calls `Inner()`; `Inner()` inserts `Customer` and `Commit()`s;
  `[IntegrationEvent] procedure OnDepEvent()` (global); `Dep Sub.HandleDep` subscribes to it and
  inserts `Customer`.
- workspace `Ws Work`: `CallChain()` calls `Lib.Outer()`; `RaiseDep()` calls `Lib.OnDepEvent()`;
  `TouchCust()` inserts `Customer`. `Ws Sub.HandleInWs` subscribes to `OnDepEvent`, inserts `Ws Flag`.

## A. What each command builds today

| probe | result | evidence |
|---|---|---|
| `prove` model | single-app only: `assemble_and_resolve_workspace_program(ws, id, false)`; no `--single-app` flag | `src/engine/l5/prove.rs:880-894`; `src/bin/alsem.rs:1124-1150` |
| `prove` builds on digest's machinery | yes: `compose_snapshot`, `resolve_changed_roots(…, primary_guid_of)`, `compute_digest_effects_cli(…, None)`, `build_db_effect_bundle` + `DbTempStates`, `build_envelope_diagnostics_json` (single-app) | `prove.rs:897-975` |
| `prove` answers `no` when the digest entry has no matching effect and clear obligations | yes (`evaluate_may_question` → `obligations_clear`) | `prove.rs:409-440` |
| `fingerprint` model | single-app: `assemble_and_resolve_workspace_program(ws, id, opts.no_roots_config)`. Full snapshot = `compose_full_snapshot(&resolved)`, which builds `build_r3a3_source_only_base(resolved)` (a single-app cone base) | `src/engine/l5/fingerprint_cli.rs:345-352, 388`; `src/engine/l5/snapshot_full.rs:66-71`; `capability_cone.rs:3881` |
| `fingerprint` has per-app concepts already | yes: `--shard primary-only|all`, `apps`, `--inventory-only` (al-perf consumes it: `docs/history/engine-migration.md:48`) | `alsem fingerprint --help`; `snapshot_full.rs:1295-1310` |
| `build_cross_app_base` takes no roots-config switch | yes, it passes `false` to `assemble_and_resolve_cross_app_program`; `fingerprint --no-roots-config` would need a parameter | `capability_cone.rs:3309-3318` |
| `diff` model | compares two snapshots; a workspace argument is recomposed with the SAME single-app `compose_full_snapshot` fingerprint uses. Snapshot files come from `fingerprint --format cbor.gz` | `src/engine/gate/diff/cli.rs:80-103, 132-151` |
| what dependencies mean for `diff` | only "each side's snapshot was built with its own deps". A persisted fingerprint snapshot vs a ws-mode side is like-for-like only if both are built in the same mode, and no snapshot records its mode today | same; `snapshot_full.rs:100-115` (no mode field) |
| `events fanout/chains` model | single-app `build_detector_context(&resolved, ALL)`; `--scope primary|all` already exists, keyed on `ix.primary_routines` = routines NOT in `dep_routine_ids` (built for cross-app) | `src/engine/gate/events.rs:364, 395, 496, 523`; `src/engine/l5/event_flow.rs:174-188, 577-590, 683` |
| `events` on a dependency event, today | `fanout` and `chains` report **0 events** on the fixture, both scopes, although `Ws Sub` subscribes to the dependency's `OnDepEvent` | `alsem events fanout fx/own/ws --format json --scope primary|all` → `"totalEvents": 0` (run in this probe) |
| `policy check` model | single-app `build_detector_context(&resolved, ALL \| RAW_INHERITED_FACTS)`; rules read inherited capability facts incl. `capability.resource.table.name` | `src/engine/gate/policy/pipeline.rs:193, 251-266`; `policy-default.yaml` |
| cross-app detector context exists for in-crate use | yes: `build_detector_context_with(resolved, demanded, Some(CrossAppInputs{dep_routine_ids, leaf_summaries, injected_typed_edges, declared_dependencies, app_versions, friends}))`, as `run_detectors_cross_app` uses it | `src/engine/l5/detector_context.rs:885-901`; `src/engine/l5/registry.rs:441-466` |
| events/policy JSON envelopes have no `payload` | yes: hand-built insertion-order `{al_sem_version, generated_at, kind, summary, entries}` / `{…, kind, policySource, policyVersion, ruleSummaries, findings, …}`; no `schemaVersion` | `events.rs:143-170`; `format_policy.rs:209-240` |
| events/policy golden fixtures have dependencies | no: every r0-corpus fixture named by `cli_c_events_differential` / `cli_c_policy_differential` has `app.json` and no `.alpackages` (two policy fixtures are inline) | loop over `tests/cli-c-{events,policy}-goldens` names vs `tests/r0-corpus/*/.alpackages` (this probe) |
| events/policy goldens | events 71 files (33 JSON, 38 txt); policy 36 (12 JSON, 10 SARIF, 14 txt) | `find tests/cli-c-{events,policy}-goldens` |

## B. What #66 used (the pattern to copy)

| probe | result | evidence |
|---|---|---|
| shared base | `pub(crate) fn build_cross_app_base(workspace: &Path, model_instance_id: &str) -> Option<R3a5CrossAppBase>`; `None` for a fail-closed workspace or no primary app | `capability_cone.rs:3309-3325` |
| mode carrier | `gate::run::AnalysisTarget::{SingleApp(Box<Model>), CrossApp(Box<R3a5CrossAppBase>)}` + `.resolved()` | `src/engine/gate/run.rs:173`; `digest_cli.rs:1554-1578` |
| `payload.dependencies` producer | `program::resolve::full::LoadedDependencies { cross_app: bool, apps: Vec<Value> }`, `to_json()` → `{mode: crossApp|singleApp, apps}`, `human_line()` → `None` when no app (so human output is mode-independent with no deps). `apps` = `R3a5CrossAppBase.loaded_apps`, made by the ONE producer `full::loaded_apps(ctx, coverage)` (also `alsem graph`'s `meta.apps`) | `full.rs:2144-2220`; commit `0fcd0203` |
| cross-app diagnostics | `compute_analyzer_diagnostics_cross_app(ws, base)` and private `analyzer_diagnostics_of_run(ws, resolved, run)` over `run_detectors_cross_app(base, &default, true)` | `run.rs:1073-1087`; `digest_cli.rs:62` |
| selectors stay on the workspace | `build_selector_indexes(snap, primary_guid)`, `is_primary_routine` | `selector_index.rs`; `effect_query_cli.rs` (commit `851e4ae1`) |
| no-dependency contract test to copy | `tests/cli/cli_digest_query_dependencies.rs::without_dependencies_cross_app_equals_single_app`: every digest + query golden argv, both formats, cross vs `--single-app`, `"mode": "singleApp"`→`crossApp` then byte-equal; `compared >= 58` floor | `cli_digest_query_dependencies.rs:376-433` |
| dependency fixture helper | `crate::symbol_app::write_source_app(path, guid, name, ver, symbols, sources, extra)` | `tests/common/symbol_app.rs:14` |
| schema bumps | digest 1.3.0→1.4.0 (now 1.6.0), query 1.0.0→1.1.0 | commit `851e4ae1` |
| #66 CDO cost (the reference numbers) | digest 8.4 s/2112 MB → 27.1 s/2828 MB cross-app; query touches 4.1 s → 17.2 s; base 18.5 s | `CHANGELOG.md:494-510` |
| CDO single-app costs of the five commands (the "default detector set" entry) | prove ~7.9 s/2070 MB, fingerprint ~7.3 s/1360 MB, events fanout/chains ~4.5 s/575 MB, policy ~4.5 s/575 MB | `CHANGELOG.md:359-375` |

## C. Soundness probes (cross-app snapshot path)

| probe | result | evidence |
|---|---|---|
| digest cross-app follows a dependency CALL chain | **No, and says coverage is complete.** `digest --routine CallChain` (calls `Outer`→`Inner`, which inserts and COMMITs): cross-app entry has `effects: []`, `coverage {status: complete, reasons: []}`, `unresolved: []`, `visitedRoutines: 2`. `--single-app`: `coverage partial, reasons [external-target]` | `out/d_CallChain.json`, `out/d_CallChain--single-app.json` |
| cause | the base's model has its calls cut to workspace callers (`resolved.calls = calls` filtered by `!dep_routine_ids.contains(&e.from)`); `compose_snapshot(base.resolved)` never sees the injected dependency typed edges, so `Outer` looks call-free and complete | `capability_cone.rs:3333-3345, 3373-3392, 3430-3433`; `digest_cli.rs:1584` |
| consequence for `prove` if switched like digest | `prove CallChain may-commit` would answer **`no`** (no effect, clear obligations) — a false proof. Today (single-app) it answers `unknown`, blocked by `unresolved-callsite Lib.Outer` | `out/p_CallChain.json`; `prove.rs:409-440` |
| `query effects CallChain` cross-app | `effects: []` (by the stated leaf contract; no uncertainty either) | `out/q_CallChain.json` |
| this is a #66 defect, not #70 work | digest (shipped cross-app in #66) already reports false `complete`. File as a discovery | above |

## D. `workspaceFingerprint`

| probe | result | evidence |
|---|---|---|
| how computed | sha256 over sorted `(kind, path, contentHash)` of `derive_inputs` + `alsemVersion`. Inputs: `app.json`, **`<ws>/.alpackages/*.app` (kind `dep-package`)**, `roots.config.json` (if loaded), `al-sem.coverage.yaml`. NOT the `.al` sources | `snapshot_full.rs:61-64, 790-875` |
| issue premise "swapping a dependency does not change it" | **False for the workspace's own `.alpackages`**: swapping the `.app` changed `47b78e50…` → `ad6fcb68…`. **True for an ancestor `.alpackages`**: `35929a77…` before and after the swap, while the cross-app run loaded that `.app` (`apps: [embedded]`) | probe run (digest `payload.workspaceFingerprint`), `fx/own`, `fx/anc` |
| a `.al` edit does not change it | confirmed (same `ad6fcb68…` after appending a comment to `src/Work.al`) — al-sem's input model; out of scope | probe run |
| closure digest | `semantic_golden::dependency_closure_digest(root) -> Result<String,String>`: every `.app` from `dependencies::discover_app_files` (own + each ancestor `.alpackages` up to 8 levels / first dir with `.git`, canonical-path dedup), keyed `"../"*ups + ".alpackages/<name>"`, length-prefixed, `closure-v2:<hex>` or `closure-v2:empty`; `Err` on an unlistable folder/unreadable file | `src/program/resolve/semantic_golden.rs:235-283`; `src/dependencies.rs:335-425` |
| is the closure digest available from the cross-app base | no field on `R3a5CrossAppBase`; it is a pure function of the workspace path, callable anywhere (the loader uses the same `discover_app_files`) | `capability_cone.rs:3240-3255`; `dependencies.rs:557` |
| consumers of `workspaceFingerprint` | digest, prove, fingerprint query JSON, full snapshot + shard manifest. **No in-repo cache keys on it** (grep of `src/`: only those emitters) | `grep -rn workspaceFingerprint src` |
| goldens pinning a value | **171 files**: `tests/cli-b-goldens/{snapshot 66+4, fingerprint 46+2, digest 21, prove 19, diff/snapshots 12}`, `tests/cli/*` 1 | `grep -rl workspaceFingerprint tests` |
| do any of those fixtures have dependencies | no `dep-package` input in any `tests/cli-b-goldens` file; no `.alpackages` at `tests/`, `tests/r0-corpus/` or the repo root; the walk stops at the repo root (`.git` is a file in a worktree, `exists()` is true) | `grep -rln dep-package tests/cli-b-goldens` (empty); `find tests -name .alpackages` |
| CDO | `Cloud/.alpackages` is the workspace's own (10 apps); `U:/Git/DO-cdo-baseline` has `.git` and no `.alpackages` → the CDO fingerprint covers its whole closure already | `ls` of both dirs |

## E. `leaf_summaries`

| probe | result | evidence |
|---|---|---|
| producer | every SOURCE-BEARING dependency routine (`body_available`) → `base_intraprocedural_summary` (its own direct effects), fixed in the solve | `capability_cone.rs:3403-3416` |
| consumers | `cross_app_db_effect_bundle` (query + digest cross-app), `run_detectors_cross_app` → detector context (analyze, digest/… diagnostics), `project_r3a5_cross_app` | `effect_query_cli.rs:205-217`; `registry.rs:456`; `capability_cone.rs:3492` |
| what a leaf changes | the solver reads callees only via `graph.edges_by_from`; a leaf is never recomputed. Dependency CALL edges are already cut from the graph, so the only out-edges a dependency routine has are **event-dispatch edges from a dependency publisher** (publisher routine → subscribers) | `summary_runner.rs:1041-1046, 1098, 1606-1613`; `combined_graph.rs:318-345`; `capability_cone.rs:3334-3347` |
| so emptying `leaf_summaries` matters only at a dependency publisher reached from the workspace | yes (by construction above). A dependency subscriber has no out-edges (its calls are cut; a dependency cannot subscribe to the workspace's events), so the issue's "dependency subscriber whose own effects differ from its solved summary" does not exist; the publisher does | same |
| #66's embedded fixture cannot discriminate | `CallLib` calls `InsertCust` (no out-edge) → leaf = solved | `cli_digest_query_dependencies.rs:89-110` |
| the discriminating shape, measured | `RaiseDep` (ws) calls the dependency publisher `OnDepEvent`; subscribers `HandleDep` (dep, inserts Customer) and `HandleInWs` (ws, inserts Ws Flag). **Cross-app `digest`** reports both `DB_INSERT`s (its cone follows the dispatch) **with no `detail.tempState`**; **cross-app `query effects RaiseDep` reports `effects: []`** (the leaf publisher holds no rows). The two commands disagree, and #65's "every DB_* effect carries tempState" is broken in cross-app mode | `out/d_RaiseDep.json`, `out/q_RaiseDep.json`; contrast `out/d_TouchCust.json` (`tempState: known(false)`) |
| predicted with `leaf_summaries` emptied (not run: needs a code change) | the publisher is solved → its summary = both subscribers' inserts → query lists them, digest's rows exist → `tempState: known(false)`; i.e. emptying makes the commands AGREE. A test that "fails when leaf_summaries is emptied" on this fixture would pin the current incoherence | reasoning from the rows above; verify as the first red test |
| "digest uses the single-app store in cross-app mode" | the realistic mutation is `build_db_effect_bundle(base.resolved)` (digest holds no single-app model in cross-app mode); over the base's cut calls that equals "no leaves", the same mutation as emptying | `digest_cli.rs:1627-1634`; `effect_query_cli.rs:223-260` |
| `cli_digest_temp_state` cannot see it | its probe workspace has no dependency (stated limit in its header); its query check only compares tempState parts that exist, so a MISSING tempState passes vacuously | `tests/cli/cli_digest_temp_state.rs:20-22, 267-330` |
