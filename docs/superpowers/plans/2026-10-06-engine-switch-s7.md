# Engine switch S7 — cross-app replacement (G11a, G15b-existing)

Spec: `docs/superpowers/specs/2026-10-06-engine-switch-design.md`, S7.
Branch: `engine-switch/s7-cross-app`, from master `819fc0c5` (S6 merged).

## What exists

- No `alsem` subcommand reaches the cross-app path. `alsem analyze` runs d13/d16/d17,
  but in the single-app context they are inert: the model holds workspace routines only
  and `dep_routine_ids`/`declared_dependencies`/`app_versions` are empty.
- The cross-app world is three layers, all L3:
  1. `cross_app_l3` builds one merged L3 model: workspace entities, then every `.app`
     under `<ws>/.alpackages` appended (ABI projection: bodyless, empty features; the
     R4 variant parses embedded source instead), then L3 `resolve`.
  2. `capability_cone::build_cross_app_base_from_cross` re-reads each `.app` twice
     (`build_dep_artifact_l4`, `recover_dep_retained`), each time re-parsing the
     embedded source into an isolated L3 model. It takes injected own->own edges (cone
     only) and per-dep-routine retained summaries (fixed solver leaves) + direct facts.
     It runs L3 `resolve_calls` and `build_event_graph` over the merged model.
  3. Consumers: `project_r3a5_cross_app` (symbol-only base; `aldump
     --r3a5-cross-app-summary`, r3a5 goldens/oracles) and `project_r4_findings_cross_app`
     (R4 base; the 4 cross-app r4 goldens d13/d16/d17). `r3a4_projection` (aldump
     `--r3a4-dep-hooks`, r3a4 golden) projects the dependency artifact products; cited
     evidence, the order index and return summaries have no other reader.
- The cross-app detector context leaves `resolved_call_edge_by_callsite`,
  `root_classifications_by_routine` and `ordering_source` empty: d40/d41/d42/d53/d55/d61,
  the ordering detectors (d47/d49/d51) and the root readers (d50/d51) are blind there.
- Program engine: the resolver is already caller-relative (closure, visibility, friends,
  shadowing). Three spots pin the caller to the primary app (`full.rs` caller object id,
  `workspace_object_map`, `resolve_object_run`), and file selection takes primary files
  only. Dependency trees exist only under `FULL` (`ctx.dep_bodies()`); every analyze
  path builds `Summary`. The detector model, adapter and event graph are primary-only.
  Internal routine ids do not hash the source-unit id, so a dependency routine projected
  with its app guid and the same model instance id gets the legacy id.

## Contract decisions (stated here, triaged where they move output)

1. **One cross-app base.** The symbol-only (r3a5) and embedded-source (R4) bases become
   one program-backed base. Source-bearing dependency routines carry their parsed
   features; symbol-only ones are bodyless. Expected: the r3a5 golden can move where a
   dependency routine now carries real features (it was empty-featured with
   `body_available` flipped). Triage.
2. **Fixed-leaf semantics kept.** A source-bearing dependency routine is a solver leaf
   holding its own direct dbEffects; dependency->dependency edges reach the cone only
   (today's injected-edge rule: own->own, Direct | Method+Resolved | Interface+Maybe,
   dedup (from,to) first-wins). A symbol-only routine stays a non-leaf with
   `opaque-body`. Growing propagation is S8.
3. **Dependency population = the program engine's** (`discover_app_files`: own and
   ancestor `.alpackages`, GUID-deduped, self-dependency skipped), not the legacy
   `<ws>/.alpackages` scan. No fixture has ancestor packages or duplicates; any move is
   triaged.
4. **Owning-app resolution.** A dependency body is resolved from its own app's view
   (its closure and visibility), never the workspace's. The edges are a separate
   result: `ProgramReport` and the north-star histogram do not change.
5. **Ledger from the program engine.** Declared dependencies (d17) and resolved versions
   come from `FreshCoverage::ledger` filtered to `declared_by` = primary.
6. **Profile.** The cross-app builder states `FULL`. The single-app analyze path stays
   `Summary`. Memory of a `FULL` cross-app build on CDO is measured, not assumed.
7. **Anchors.** Dependency source-unit ids are `dep:<guid>:<path>` with the program's
   percent-decoded path; files ordered by path. Ids do not change; an anchor with `%xx`
   in a zip entry name would (no fixture has one).

## Steps (one commit each)

- **S7.0 Baseline.** `aldump --r4-findings-cross-app <ws>` on the OLD path (all
  registered detectors, cross-app mode), so CDO/DO/fixtures get a before/after for the
  cross-app findings, plus `--r3a5-cross-app-summary` dumps. Saved under the harness
  root as label `s7-0`.
- **S7.1 Owning-app body resolution.** `full.rs`: the caller app becomes a parameter
  (`caller_app`), an object map per owning app, `resolve_object_run` takes it; a new
  `resolve_dependency_bodies(ctx)` (needs `FULL`) resolves every dependency unit's files
  with that unit's app as caller. Tests: a dependency calls its own internal procedure
  (resolves) and a workspace procedure (does not: not in its closure); discrimination by
  pinning the caller back to the primary. CDO: count and taxonomy of dependency-caller
  edges, reported only.
- **S7.2 Cross-app model population.** `assemble_cross_app_program(ws, mi)`: the
  primary rows as today, plus each source-bearing dependency file projected whole-file
  (`dep:` unit id), plus symbol-only dependency routines/objects/tables from the ABI
  nodes (bodyless, record-parameter variables as the ABI projection makes them). Test:
  ids, stable ids and attributes equal the legacy merged model's on the r3a5 and r4
  cross-app fixtures (row multiset compare).
- **S7.3 Calls and events over all apps.** The adapter takes sites for every app in the
  model with app-qualified routine keys; the event graph includes dependency publishers
  and subscribers. Test: a dependency->dependency call and a dependency subscriber appear
  with program-engine targets.
- **S7.4 Program-backed cross-app base.** `build_cross_app_base` from the S7.2/S7.3
  model: `dep_routine_ids`, leaf summaries, direct facts/coverage, injected cone-only
  edges (decision 2), ledger fields. `project_r3a5_cross_app` and
  `project_r4_findings_cross_app` switch; `recover_dep_retained` and the two-base split
  are deleted. Goldens r3a5 + 4 cross-app r4 triaged. r3a5 oracles O1-O7 stay green.
- **S7.5 Dependency artifact products.** The R3a-4 products (intra-app edges, cited
  evidence, order index, return summaries, freshness stamp) are projected from the
  program-backed model of the requested app's full required population.
  `build_dep_artifact_l4`'s producer and `DepIdStabilizer`'s re-parse are deleted;
  r3a4 golden/oracles/vectors triaged.
- **S7.6 Cross-app detector context completion.** Fill
  `resolved_call_edge_by_callsite`, `root_classifications_by_routine` and
  `ordering_source` from the cross-app model. Findings that newly appear on the
  fixtures and CDO/DO are triaged against source.
- **S7.7 Docs.** CHANGELOG, spec as-built, OUTSTANDING, CLAUDE.md, memory.

## Out of scope

- The L3-measuring modes (`--l3-cross-app`, `--l3-*-cross-app`), `cross_app_l3` itself,
  the r2-5b goldens and the smoke tests: they measure L3 and go in S9.
- Making `alsem analyze` cross-app (live d13/d16/d17): an owner decision; it belongs
  with the S8 world growth.
- d43/d45 decisions (S8).

## As built (2026-10-06)

Commits: S7.0 `f66d4c00`, S7.1 `5fa55aca`, S7.2 `eae331e5`, S7.3 `8fdb1d33`,
S7.4+S7.5 `03489d02` (one commit: the base consumes the artifacts), S7.6 `72c59439`.
Decision 3 changed in S7.4: the population is the workspace's REQUIRED dependency
closure, not every loaded app (DO's test app depends on the workspace). Decision 5
changed: d17's declared list and versions come from the snapshot (the ledger's
source), keeping the legacy guid spelling. The CDO/DO triage reports are summarised
in the CHANGELOG; open detector limits in `docs/OUTSTANDING.md`.
