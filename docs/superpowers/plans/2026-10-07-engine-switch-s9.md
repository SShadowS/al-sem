# Engine switch S9 — delete L3 and rename

Spec: `docs/superpowers/specs/2026-10-06-engine-switch-design.md`, S9.
Branch: `engine-switch/s9-delete-l3`, from master `78466762` (S8 merged).

## What the inventory found (2026-10-07, file:line evidence in the S9 inventory)

1. **Production still calls an L3 resolver.** The `alsem analyze` adapter calls
   `implicit_edges::implicit_trigger_edge_for_op` for every trigger-capable record
   op (`program_calls.rs:1881`), only to fill two census counters. It is the reason
   the adapter builds a full `SymbolTable` (`:1012`).
2. **Two production fallbacks parse through L3's disk assembly:** a program parse
   missing a file (`program/model/workspace.rs:1825-1828`), and analyze's failure
   classification (`gate/run.rs:249-270`).
3. **The r4/r4f helper is not the production model.**
   `assemble_and_resolve_workspace_with_program_calls` (`program_calls.rs:696-707`)
   builds L3's disk model and then attaches program calls.
4. **Goldens that look program-backed hold L3 resolution.** A model built with
   `precomputed_calls/_events = None` makes `calls_for`/`events_for` fall back to
   `resolve_calls`/`build_event_graph` (`call_resolver.rs:667-675`,
   `event_graph.rs:230-238`). This covers r3a1/2/3, `l4-summary-baseline`,
   `perf_bounds`, `cli_b_diff`, `cli_b_fingerprint`, `cli_a_stats`, and every gap and
   temp_state test. Switching their builder moves them.
5. **No inline program builder exists.** 70+ tests use the L3 inline builders.
6. **The semantic-edges goldens can only be re-minted through L3** (`l3_mint`,
   `mint-goldens.rs:59`, `program_resolve_harness.rs:3329`).
7. **`SymbolTable` stays in half.** Record typing needs its object/table half
   (`build_without_routines`); only the routine half is resolver-only.

## Steps (one commit each, task gate green, every moved golden triaged)

- **S9.0 Compiler oracle for the semantic-edges goldens** (owner decision 2026-10-07,
  R1). The AL extension ships `altool graph` (Microsoft.BusinessCentral.CallGraph): the
  compiler's own call graph as JSONL shards (`node`/`edge`/`pub`/`sub`/`icall`/`impl`
  rows; edges carry caller, callee, kind `Direct`/`Trigger`/..., confidence
  `Resolved`/`OverApprox`, source path and line). It is the reference only; the product
  never depends on it.
  - S9.0a Probe and pin: the extension version, `extract-whole` over CDO plus its
    dependency sources (a single-app `extract` drops cross-app calls), the shard
    schema, determinism across runs, cost.
  - S9.0b Map compiler rows to our canonical site keys (file + line; the semantic
    goldens already ignore columns) and targets (object + member + overload).
  - S9.0c Three-way compare on CDO: compiler vs frozen L3-minted goldens vs program
    resolver. Triage every disagreement as our bug (fix it at the root), a compiler
    graph limit (recorded, e.g. `OverApprox` triggers), or a mapping defect.
  - **S9.0c status (2026-10-07): the four workspace PROGRAM BUG causes are fixed**
    (`docs/s9-oracle/cdo-workspace-triage.md`): RunTrigger (`2e7ca179`, S8+S10),
    parens-less calls in expressions (`0eae46f0`, S1), ternary/`is`/`as`/list lowering
    (`ca1e8b0d`, S3). Workspace oracle after: 6,878 pairs agree, 167 compiler-only,
    258 program-only; every residual pair is a COMPILER LIMIT or MAPPING shape.
    **Open, found by `--all-apps`:** dependency bodies resolved from their own app
    (`resolve_dependency_bodies`, what cross-app analyze reads) carry ~8,000 `Unknown`
    routes in 431,248 edges on CDO (UntrackedReceiver ~2,600, CatalogMiss ~3,340,
    CompoundReceiver ~930, MemberNotFound ~615, ObjectNotInGraph 134,
    AccessFilteredOverload 77, ...). `realUnknownRate` never measures them: it covers
    workspace and publisher edges only. Most dependency-wide oracle disagreements are
    these.
- **S9.0e Dependency-body unknowns to zero** (owner decision 2026-10-07: part of S9).
  Baseline on CDO: 6,315 unknown edges of 431,248 (7,880 unknown routes:
  catalogMiss 3,341, untrackedReceiver 2,617, compoundReceiver 929, memberNotFound
  615, objectNotInGraph 134, reportRecExcluded 87, accessFilteredOverload 77,
  overloadAmbiguous 41, receiverOutOfClosure 33, arityMismatch 3,
  internalNotVisible 3). Measured with `aldump --dependency-bodies-stats --sites`;
  held by the `dependency_body_unknown_ceiling_on_cdo` ratchet, lowered with every
  fix. One family per commit, each root-caused against real source.
  - **Status 2026-10-07 (HEAD `a51126c1`): 6,315 -> 4.** Merged to master once at
    643 (`d389d984`, local, not pushed). Every rule since carries an alc 18.0.41.45789
    probe (control fails with AL0122/AL0118/AL0133) and a discrimination proof; see
    CHANGELOG `### Fixed` top entries. Workspace stayed 0 unknown / 23
    `ambiguousResolved` throughout; dependency `ambiguousResolved` 863 -> 710.
  - **The 4 left** (`target/cdo/aldump.exe --dependency-bodies-stats --sites
    U:/Git/DO-cdo-baseline/Cloud`):
    1. FIXED (4 -> 3): arityMismatch `MfgCalculateBOMTree.Codeunit.al:207` (Base
       App), a procedure header split across `#if not CLEAN27` / `#else`. Each arm is
       now its own routine, its body lowered with the symbols its condition decides.
       It raised dependency `ambiguousResolved` 710 -> 716 (calls seeing both arms
       of one routine: `AccScheduleOverview.Page.al:2130/2139`, Continia Core
       `CoreSessionManager.Codeunit.al:77/86` twice). FIXED (716 -> 710): overload
       selection now drops candidates outside the call's build (its `#if` branches
       and its routine's arm), and a ratchet pins dependency ambiguous <= 710.
    2-3. FIXED (3 -> 1): compoundReceiver `ReconcileCustandVendAccs.Report.al:507/535`,
       `QueryVar.EntryType.AsInteger()`. A plain query column (no `Method`) now types
       as its source field.
    4. FIXED (1 -> 0): untrackedReceiver `ImportExportWorkflow.XmlPort.al:226`,
       `EventConditions.AddText(..)`. XmlPort text nodes are now `Text`/`BigText`
       xmlport globals. **S9.0e is done: 0 dependency-body unknown edges on CDO.**
  - Answered: no S9.0e lowerer fix needs a cache-version bump. The only cache this
    engine writes is `snapshot/cache.rs` (raw extracted `.al` text, keyed by the
    `.app`'s blake3, before parsing). The R3a-4 dependency-cache artifacts that
    `cache_prune.rs` versions are only read and pruned; nothing in `src/` mints one.
  - S9.0d `mint-goldens` mints from the compiler graph (anonymized as today, stamped
    with the extension version); the in-repo fixture golden too. `l3_mint` is then
    unused and goes with L3 in S9.6.
  - **S9.0d done (2026-10-08).** `scripts/compiler-graph` + `mint-goldens
    --compiler-graph` write `cdo-compiler-anon.json` (7,045 pairs) and
    `fixture-compiler-anon.json`. The CDO audit pins every disagreement by rule
    (`compiler_golden::Verdict`): 6,878 agree, 369 explained, 0 unexplained. The
    L3-minted goldens, the adjudication overlay and their audits are deleted.

- **S9.1 Cut the production legacy calls.** Remove the adapter's L3 trigger
  comparison and its counters; the adapter's `SymbolTable` becomes table-only.
  Replace both disk-assembly fallbacks with program-side answers. Point aldump
  preconditions at the program model. Expected: zero goldens move.
- **S9.2 Inline program builder.** Files + app id -> temp workspace ->
  `assemble_and_resolve_workspace_program`, with a test that it equals the disk path.
  Decide the model-instance id and unit-id spelling.
  - **Done (2026-10-08):** `program_calls::assemble_and_resolve_inline_program(files,
    app_guid, model_instance_id)` and its `_default` (`r0`). Decisions: the
    model-instance id stays a parameter with the same `r0` default; unit ids are
    `ws:<relative path>`, exactly the L3 inline spelling, because the files are
    written at the paths given. Two differences from the L3 inline builder that S9.5
    will meet: `primary_app` is set (the builder writes an `app.json`), and calls and
    events are the program engine's. `the_inline_program_model_is_the_disk_model`
    checks every single-app r0-corpus fixture (rows, calls, events, root
    classifications) against the disk build.
- **S9.3 The r4/r4f helper uses the production builder.** Triage any move (expected
  none if S2b.4's census holds). **Done (2026-10-08): zero goldens moved.**
- **S9.4 Move the keepers out of `engine/l3`** (pure moves): the adapter
  (`program_calls`), `event_param_temp`, binding helpers, `coverage` (minus
  `project_coverage_cross_app`), `calls_for`/`events_for`/`isolated_event_ids`.
  - **Done (2026-10-08), partly by design.** Moved to `program::model`:
    `program_calls`, `event_param_temp`, the binding helpers and
    `object_run_dispatch_kind` (into `calls`), `isolated_event_ids` (into `events`);
    the old `engine::l3` paths re-export them until S9.7. **Not moved:**
    `calls_for`, `events_for` and `coverage`. Their `None` fallback is L3's own
    `resolve_calls`/`build_event_graph`, so in `program/` they would break the S1
    guard (`program_has_no_legacy_engine_imports`: the program engine never
    imports L3). They move in S9.6, when `precomputed_calls/_events` become
    mandatory and the fallback goes. Same reason: the adapter tests compare with
    `resolve_calls`, so their file is `engine/l3/program_calls_adapter_tests.rs`
    (a `#[path]` child module of `program_calls`); S9.6 reworks or deletes them.
- **S9.5 Switch every test that uses L3 only as a model builder**, family by family,
  each with a golden triage: gap, temp_state, src unit tests, cli_b_*, cli_a_stats,
  d1_downgraded, cli_p1, r3a1/2/3 (goldens move), l4_summary_differential (re-freeze),
  perf_bounds. Replace r2a and r2d with program-backed goldens; add a program
  event-graph golden in place of r2c.
  - **Census (2026-10-08):** pointing every L3 builder at the program builder
    moved 25 tests. Not moved at all: temp_state, `l4_summary_differential` (no
    re-freeze needed), every gap test but G-18's stated collision. Moved:
    the 3 stated-collision tests; `cli_a_stats` and `cli_b_fingerprint` on
    `ws-d35` only (numeric `ObjectType::Codeunit, 50` subscriber targets: L3
    dropped them, the program engine keeps them as `unknown`); r3a1/2/3
    differential goldens; the r2d coverage golden; the r3 and `tests/l3` vector
    tests (file names without `.al`). `tests/l3` measures L3 itself and goes in
    S9.6; `perf_bounds` is release-only and was not in the census.
  - **S9.5a done (2026-10-08):** l5 unit tests, gap, temp_state,
    d1_downgraded (50 files); the 3 collision tests re-key the precomputed
    edges.
  - **S9.5b-e done (2026-10-08):** cli stats/diff/fingerprint (b); r3a1/2/3
    differentials, oracles, vectors (c, after two overload fixes the census
    exposed: enum value arguments, exact-vs-conversion and the sole-applicable
    rule); l4_summary_differential re-frozen and perf_bounds (d); r2a and r2d
    on the production model, and r2c projects the program engine's event
    graph (e). **Decision:** r2c is not deleted in S9.6 — it IS the program
    event-graph golden now (the stable projection moved to
    `program::model::events`); its `.l3eg` file names are legacy, like the
    `L3*` types. Still on L3's builders, deleted with L3 in S9.6: `tests/l3`,
    r2b (`project_call_graph`), `r3a0_unfetched_dep_opaque`, `aldump_smoke`'s
    event-graph emitter test.
- **S9.6 Delete the legacy engine:** resolver chain, event-graph builder and
  projections, `implicit_edges`, `receiver*`, `static_arg`, `type_ref`, `type_rel`,
  `al_type`, `al_builtins`, `member_builtins`, `resolution_class`,
  `call_graph_projection`, `b3_diff`, `l3_mint` (per decision R1),
  `deps/cross_app_l3.rs`, the L3-parse half of `workspace.rs`, the routine half of
  `SymbolTable`, the L3 aldump modes, `tests/l3`, r2b, r2-5b-*, `b3_triage_r0`,
  `r3a0`, `docs/b3-triage/*.md`. `precomputed_calls/_events` become mandatory.
  `scripts/check-goldens`, the pre-commit hook and CLAUDE.md change with it.
- **S9.7 Rename** the `L3*` model types and `l3_workspace`; remove `engine/l3/mod.rs`
  and the `engine::l2` alias. Mechanical; zero goldens move (no type name is
  serialized).
- **S9.8 Acceptance inventory:** `rg` finds no `resolve_calls`, `build_event_graph`,
  `implicit_trigger_edge_for_op`, `assemble_l3_workspace_from_disk`, `cross_app_l3`;
  `src/engine/l3` is gone; every cross-app context field mapped (G14 includes
  `abi_ingest_errors` "gets a reader": verify).

## Decisions

- **R1 — semantic-edges goldens: DECIDED** — the compiler oracle (S9.0).
- **R6 — `merged_index`: DECIDED** — retire it (its aldump mode and goldens); keep
  `projection.rs` and the `abi_native`/`attr`/`stable_id` vectors.

## Risks

- Goldens pinned on L3 calls move when their builder switches (R2): line-level triage,
  never a bulk regen.
- Population: L3 inline/disk is whole-file; production is the program's physical rows
  (R3). Ids in inline tests may change with a temp-dir builder (R4).
- `scripts/` and CLAUDE.md change on this branch (R8): a human-driven branch.
