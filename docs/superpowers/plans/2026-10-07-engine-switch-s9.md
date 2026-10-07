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
    these. Owner decision needed: a dependency-body resolution arc inside or after S9.
  - S9.0d `mint-goldens` mints from the compiler graph (anonymized as today, stamped
    with the extension version); the in-repo fixture golden too. `l3_mint` is then
    unused and goes with L3 in S9.6.

- **S9.1 Cut the production legacy calls.** Remove the adapter's L3 trigger
  comparison and its counters; the adapter's `SymbolTable` becomes table-only.
  Replace both disk-assembly fallbacks with program-side answers. Point aldump
  preconditions at the program model. Expected: zero goldens move.
- **S9.2 Inline program builder.** Files + app id -> temp workspace ->
  `assemble_and_resolve_workspace_program`, with a test that it equals the disk path.
  Decide the model-instance id and unit-id spelling.
- **S9.3 The r4/r4f helper uses the production builder.** Triage any move (expected
  none if S2b.4's census holds).
- **S9.4 Move the keepers out of `engine/l3`** (pure moves): the adapter
  (`program_calls`), `event_param_temp`, binding helpers, `coverage` (minus
  `project_coverage_cross_app`), `calls_for`/`events_for`/`isolated_event_ids`.
- **S9.5 Switch every test that uses L3 only as a model builder**, family by family,
  each with a golden triage: gap, temp_state, src unit tests, cli_b_*, cli_a_stats,
  d1_downgraded, cli_p1, r3a1/2/3 (goldens move), l4_summary_differential (re-freeze),
  perf_bounds. Replace r2a and r2d with program-backed goldens; add a program
  event-graph golden in place of r2c.
- **S9.6 Delete the legacy engine:** resolver chain, event-graph builder and
  projections, `implicit_edges`, `receiver*`, `static_arg`, `type_ref`, `type_rel`,
  `al_type`, `al_builtins`, `member_builtins`, `resolution_class`,
  `call_graph_projection`, `b3_diff`, `l3_mint` (per decision R1),
  `deps/cross_app_l3.rs`, the L3-parse half of `workspace.rs`, the routine half of
  `SymbolTable`, the L3 aldump modes, `tests/l3`, r2b, r2c, r2-5b-*, `b3_triage_r0`,
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
