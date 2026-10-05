# Engine switch: one engine, L3 deleted — design (rev 2, 2026-10-06)

Rev 2 incorporates review round 1 (gpt-6.1-sol, 11 findings, all accepted; four
spot-checked in source). Changes from rev 1 are summarised at the end.

## Goal

Every consumer of the legacy L3 model (`src/engine/l3/`) reads the program engine
(`src/program/`, `src/snapshot/`) instead, and L3 is deleted. One engine to maintain.
This finishes what `2026-10-04-compact-graph-core-design.md` §7 started (Phase A swapped
only the call graph) and folds in its Phases B and C.

**Done means:** all targets and features compile with `src/engine/l3` deleted; no legacy
resolution entry point survives under another name (an adapter that still contains a
resolver is not completion); every regeneration path (goldens, builtin catalog, minted
fixtures) runs.

Trigger: #57. On CDO the detectors' event graph (built by L3, source-only) has 56
subscriber edges, 54 `unknown` because the publisher is in a dependency, and the event
index keeps only `resolved` edges (`event_flow.rs:206`).

## What exists (census, 2026-10-06)

- 124 source files outside `l2`/`l3` and 125 test files read L3/L2 types.
- `alsem analyze` builds L3 (`gate/run.rs:232`) and attaches program calls (`:246`).
  Everything else builds plain L3 with L3's resolver: `prove`, `digest`, `fingerprint`,
  `diff`, `events`, `policy`, `query` (direct `resolve_calls`),
  `compute_analyzer_diagnostics`, every cross-app builder, `format_html` and an L4
  projection (`capability_cone.rs:3502`) that each rebuild the event graph, nested
  rebuilds (`summary.rs:838`, `combined_graph.rs:1043`), `dep_artifact_l4`,
  `r3a4_projection`, every `aldump` L3 mode.
- The L2 walker (`l2_workspace.rs:130 build_proutine`) consumes `al_syntax` IR and needs
  no L3-assembled state as INPUT. But its raw output is not the detector model: L3
  assembly enriches it (see G1).
- The program engine depends on legacy code in: `abi_ingest.rs` (`al_attributes`),
  `resolve/builtins.rs` (`global_builtins` oracle + regeneration target), `l3_mint.rs`
  (an unconditional module that runs L3), and `src/dependencies.rs:260-287` (Microsoft
  tier constants from `engine::deps::cross_app_l3`).

## Contracts and gaps

**G1 — Body pipeline.** The moved pipeline is the walker PLUS every L3 enrichment, in a
pinned order: object-global record promotion and shadowing (`l3_workspace.rs:920-1016`),
global argument-binding repair (`:1103-1194`), RecordRef temp state from `Open`/
`GetTable` (`record_types.rs:266-370`), implicit `Rec`/`xRec` typing, every temp-state
override (`record_types.rs:372-484`), entry-temp guards. Shared immutable scalar globals
stay shared; mutable record state stays per routine. Operation ordering is applied where
L3 applies it today (L2's projection applies it inside `build_proutine`, L3 assembly
does not): the timing is part of the contract.

**G2 — Record typing** needs the table/object facts of G3/G4 (`record_types.rs:134-264`),
so those land in the same step (S2b), not later.

**G3 — Table model:** field number/class/type/blob-like, keys, temporary,
`physical_table_id`, declaring object and app provenance, real table vs extension stub,
extension-field merge with its first-wins field-number rule (`extension_fields.rs:29-86`).

**G4 — Object properties:** page type, subtype, inherent commit behaviour, single
instance, editable/*Allowed, page controls, source table (+ temporary), object anchor.

**G5 — Population and order contract.** Legacy ingestion is path order then document
order (`l3_workspace.rs:1373-1431`); the symbol table is last-wins (`symbol_table.rs:
127-182`); program assembly sorts and dedups first-wins (`build.rs:293-311, 615-716`).
Disk L3 excludes nested apps (`l3_workspace.rs:1537-1556`); program discovery includes
them (`provider.rs:30-39`). A third discovery policy exists: `engine/snapshot.rs`
(the R0 identity snapshot) has its own `discover_al_files` (skips `.alpackages`,
`.git`), `read_root_app_guid` and `count_app_json` (skips `node_modules`,
`.alpackages`), unlike `source_text::SKIP_DIRS` — found in S1, left unchanged there
because unifying it is a policy change. The new builder keeps source occurrence and ingestion order
separately from lookup order, states its duplicate-object/-routine/-field policy, and
the harness compares populations as ordered rows or multisets, never maps keyed by
identity alone. Nested-app and unreadable-file behaviour is decided per consumer and
any change is triaged. Routine metadata: `kind` taxonomy (incl. `event-publisher`), full
parsed attributes, body availability, enclosing-member range, originating object,
record parameters.

**G6 — Three identities, not one mapping.**
- *Physical declaration identity*: one per source occurrence, kept BEFORE dedup
  (preprocessor alternatives, nested XMLport members, same-id rows).
- *Semantic routine identity*: `RoutineNodeId` (excludes return type; includes the
  folded, NOT legacy-unescaped, enclosing-member text — `sig_fp.rs:133-176`).
- *Durable exported identity*: the existing encoders in `ids.rs:179-218, 266-341`, run
  over their ORIGINAL normalized inputs (return-aware signature, once-unescaped member,
  conditional member discriminator — `scope.rs:81-140`).
Mappings may be one-to-many and refuse an ambiguous single-target conversion; nothing
keyed by `RoutineNodeId` may stand in for a durable id (`decl_surface.rs:121-153` is
last-wins by `RoutineNodeId`). Hand-stated fixtures: return-type alternatives, escaped
member names, XMLport nesting, numberless objects, deliberate collisions. Physical-row
identity is NOT deferred to S8.

**G7 — Event inventory, not event edges.** Program event resolution returns the whole
subscription inventory: bound, conditional/manual, ambiguous, orphaned and
publisher-unresolved subscriptions (today dropped or recorded aside —
`index.rs:256-389`), each with element filter, attribute occurrence and evidence. The
detector event graph projects this inventory plus publisher metadata (`EventSymbol`: id,
signature hash, parameters, kind, element, `isolated`). Unresolved subscriptions stay
visible (they drive fanout coverage, `event_flow.rs:320-339`) but are excluded from
proven-subscriber indexes. Discrimination test: dropping an unresolved subscription
changes coverage.

**G8 — Adapter completion (own step, S3).** Remove the per-call AND per-operation
fallbacks (`program_calls.rs:742-821`). Define conversion for every route, condition,
completeness state and unknown reason: no first-route truncation for multicast
(`:952-982`), no dropped dependency interface routes (`:1274-1292`), interface metadata
from the program substrate rather than the L3 symbol table (`:1294-1320`), trigger
applicability (`RunTrigger`, `Validate` field selection, `:1363-1420`) owned by the
program resolver. Exit gate: SITE accounting AND ROUTE accounting (every counter,
including callee-outside-model, dropped routes, multi-route, duplicate spans), not span
pairing alone. The audit also covers DOWNSTREAM route selection: the detector context
keeps one resolved edge per call site (`detector_context.rs:1193-1202`) and d1 takes the
first matching seed edge (`d1_graph.rs:227-250`). Each such selection is either named as
deliberate detector policy or given a discriminating multi-route test.

**G9 — Coverage** is derived from the snapshot/source inventory, physical routine/body
metadata, converted route outcomes and dependency status — NOT from
`ProgramReport.coverage`, which counts obligation sets and is accumulated from returned
edges (`full.rs:181-195, 846-898`). `AnalysisCoverage` keeps its multiset semantics
(`coverage.rs:86-247`). Obligation coverage stays a separate metric. An independent
inventory check of body sites and subscriptions proves nothing vanished.

**G10 — Root classification + `roots.config`** re-keyed off the new model.

**G11 — Cross-app** (S7 + S8 below).

**G12 — Shared helpers to neutral homes:** `al_attributes`, `global_builtins` (and the
regeneration target in `builtins.rs:35-39`), `record_types`, the temp rule, `scope`
temp helpers, `operation_order`, `strip_quotes`, discovery helpers, Microsoft tier
constants. `l3_mint` moves to migration-only tooling outside guarded program modules.

**G13 — Site keys.** Internal positions are zero-based UTF-8 BYTE coordinates on both
sides (`node_util.rs:20-37` returns byte columns despite its name); no column
conversion here — LSP encoding stays its own boundary. Site keys carry owning app,
physical caller occurrence, virtual path, full span and callee fingerprint (two apps
can both contain `src/Main.al`). Program sites link explicitly to L2's DFS-numbered
`/csN`/`/opN` body sites (statement trees, bindings, witnesses and fingerprints key on
those), keeping every associated edge (today duplicate spans keep the first,
`program_calls.rs:386-396`) and operation/call roles. Tests: duplicate paths across
apps, Unicode before a site, nested calls, shared operation/call anchors.

**G14 — App ledger:** primary app identity, declared dependency requirements and
minimum versions, fetched and resolved versions, source/trust status, implicit Microsoft
dependencies, infrastructure diagnostics (`cross_app_l3.rs:65-83`,
`detector_context.rs:1820-1833`, `l3_workspace.rs:1672-1710, 1802-1825`).
`abi_ingest_errors` gets a reader.

**G15a — Dependency declaration and analysis-readiness metadata (lands in S2b, before
S3/S4).** Source summaries and ABI ingestion retain what durable ids, argument bindings
and a complete publisher/subscriber inventory need: parameter names and temp markers,
source-table/page-control metadata, subscriber attributes (`node_extract.rs:191-211`,
`abi_ingest.rs:456-534`, `decl_surface.rs:24-72`), keeping the `Missing`/
`CollapsedUntrusted` guards. Body EXISTENCE is separated from analysis READINESS, with
four states: analyzed-clean, recovered, bodyless (symbol-only), and
source-present-but-not-analyzed. A not-analyzed body is an explicit unknown boundary,
never a clean empty body — `body_available = true` with an empty operation list must not
reach `summary_runner.rs:354-397` or `capability_cone.rs:531-548` as "no effects", and
`body_available = false` must not be used to fake it (that lies to coverage). A recovered
or absent body never proves effect absence or normal return (#22's `opaque-body` is the
effect-summary half). The dependency TARGET REGISTRY (declarations, routes' targets,
boundary states) is separate from the active detector routine population, so adding
metadata does not enable dependency-body analysis before S7/S8. Fixtures: a workspace
call into a source-bearing dependency whose body is not yet projected (must not prove
"no effects"); a symbol-only publisher with named parameters and structured event
attributes.

**G15b — Dependency body analysis (S7/S8).** Hydrate and analyse bodies; propagate effects.

## Approach

Keep the detector-facing model types for now (renamed in S9) and change WHO BUILDS them:
one builder, fed only by the program engine. Detectors and L4 do not change in the early
steps, so each step's differences are attributable to the substrate.

## Harness (S0)

Extends the B3 harness (`b3_diff.rs`) to compare, between old and new builder: full
findings (all detectors, both scopes), diagnostic CONTENTS, detector statistics, scope
membership, fingerprints, every consumer's output (incl. exit status), and the model
(ordered routine/object/table/event rows, coverage, ledger). Two baselines: the previous
merged step (isolates each step) and the frozen pre-switch legacy output (cumulative
triage). Every new guard gets a discrimination proof at its production call site
(CLAUDE.md doctrine).

## Steps

Each step: own branch, merged alone, harness over CDO, DO, 8020 and every fixture; bar =
zero unexplained differences, zero regressions, written golden triage.

- **S0 — Harness** (above).
- **S1 — Helpers out** (G12). Guard: `src/program` and `src/dependencies.rs` import
  nothing from `engine::l3`/`engine::l2`/`engine::deps::cross_app_l3`. Zero goldens move.
- **S2a — Parse sharing.** The L3 projection entry takes borrowed `ParsedFile.file`/`text`
  from `ProgramContext`, and explicitly reproduces each old consumer's selected file set
  and failure policy (program discovery includes nested apps and propagates read errors,
  `provider.rs:30-39`; legacy L3 excludes nested apps and skips unreadable files,
  `l3_workspace.rs:1540-1560`). Same population, passes and order: byte-identical
  output. Intentional policy changes land separately.
- **S2b — Program-backed model assembly** (G1-G6, G13, G15a). The model is assembled from
  program nodes + the moved body pipeline. Every populated model field compared. L3
  assembly leaves the analyze path.
  S2b is cut into sub-steps, each its own commit with a harness comparison:
  - **S2b.1 — Body pipeline into the program engine.** `src/engine/l2` moves to
    `src/program/body` (pure move). `engine::l2` stays as a re-export alias
    (`pub use crate::program::body as l2`) so the 92 files that name it keep
    compiling unchanged; the alias and every `engine::l2` path are removed in S9.
    Byte-identical.
  - **S2b.2 — Model types and assembly into the program engine.** The detector model
    types and the assembly passes (`l3_workspace` model half, `record_types`,
    `extension_fields`, and what they need) move to `src/program/model`, the same
    way. Byte-identical.
  - **S2b.3 — Physical rows and occurrence order (G5, G6).** The program graph keeps
    one physical declaration row per source occurrence, before dedup, with its
    ingestion order, and the durable-id mapping (one-to-many, ambiguity refused).
    Additive: a census proves every model row maps to exactly one physical row.
  - **S2b.4 — Model rows from program rows.** Objects, tables and routines are minted
    from the physical rows (plus G3/G4 metadata on program nodes), body facts from
    the body pipeline. The IR-object walk in `project_ir` stops being the source of
    the population. Byte-identical, or triaged.
  - **S2b.5 — Dependency registry and readiness (G15a).** Separate from the detector
    population; no output change.
  - **S2b.6 — Site links (G13).** Program `SiteId` ↔ body `/csN`/`/opN`, every edge
    kept, roles kept; consumed by S3.
- **S3 — Adapter completion** (G8). Fallbacks removed; route accounting gate. Every
  dependency route keeps its target identity and boundary state (G15a registry) without
  traversing its body.
- **S4 — Event inventory** (G7), consuming G15a's complete publisher/subscriber
  declaration metadata. Claim for this step: dependency publishers bind, d44
  sees them. Its new findings are triaged against real CDO source before merge. d45's
  external-publisher behaviour needs a scope decision (its publisher must be modelled
  with a summary and it gates on `primary_routines`) — decided here, implemented in S8.
- **S5 — Coverage, roots, ledger** (G9, G10, G14), including real dependency-role
  attribution in production scope filtering (today `gate/run.rs:343` passes
  `|_obj_id| false`).
- **S6 — Every other consumer**, one subcommand per change, nested rebuilds and error
  paths included: prove, digest, fingerprint, diff/snapshot, events, policy, query,
  `compute_analyzer_diagnostics`, `format_html`, the L4 projection's event rebuild,
  aldump modes.
- **S7 — Cross-app replacement, INCLUDING the existing dependency products** (G11a,
  G15b-existing). Today's cross-app base already builds dependency artifacts and
  retained facts by re-parsing dependency bodies (`capability_cone.rs:2984-3073,
  3170-3310`, `dep_artifact_l4.rs:339-370`), and the program resolver resolves workspace
  call sites only (`full.rs:825-841`). So S7 introduces owning-app body resolution (a
  primitive, not yet enabled for new transitive analysis) and program-backed production
  of dependency direct facts, intra-app edges, ordering facts, return summaries and cited
  evidence, replacing BOTH `build_dep_artifact_l4` and `recover_dep_retained` from the
  same program parses. The existing fixed-leaf / injected-edge semantics are preserved
  or explicitly triaged. Standalone artifact builds use the requested app's full
  required population. The cross-app detector context gains the ordering input and
  resolved-site indexing it lacks (`detector_context.rs:1542-1555, 1718-1729`). Not a
  hybrid: no legacy artifact producer survives this step.
- **S8 — Phase C expansion** (G11b, G15b-new). The analysed world and propagation grow
  beyond the prior cross-app contracts, under `FULL`. Demand policy per detector, with
  seeds and traversal directions stated: forward from workspace routines for effects;
  for d43, REVERSE from subscribed-to dependency events to the dependency call sites
  that raise them, plus those sites' effect dependencies (forward traversal from a
  subscriber never reaches the raiser). d43/d45 decisions implemented here. Measure
  cold/warm peak, retained memory, allocations and old/new coexistence separately.
- **Deferred — demanded-trees profile.** Not part of the switch; `FULL` is the profile
  for S7/S8. If measured memory later requires it, it is a separately gated step whose
  design must first state: how complete forward AND reverse demand is discovered before
  an omitted body can affect an answer (e.g. a bounded discovery pass then hydration from
  the same immutable snapshot — demand discovery is otherwise circular, since an
  event-only subscriber's raiser lives in another dependency file); that declaration,
  subscription and recovery inventories stay complete; that APIs distinguish
  not-demanded from bodyless and fail loudly on undeclared reads; and that body storage
  is root-local over the shared declaration tier, keyed by the exact retained set, or a
  correctly keyed per-file tree cache — a request never silently accepts a cached subset
  (today `dep_cache.rs:222-270` keys only `keep_bodies`, and a shared-tier hit skips
  parsing, `full.rs:1252-1277`). Gate: for the same demand, answers equal `FULL`; tests
  cover different roots/detector sets, concurrent builds, retained-set growth, and an
  event-only subscription whose raiser is in another dependency file. Dropping trees
  after a `FULL` build does not lower the build peak.
- **S9 — Delete and rename.** Remove L3 resolver, assembly, symbol table, event graph,
  `cross_app_l3`, L3-only aldump modes. Golden families that pin only the deleted engine
  (`r2a-d`, `r2-5b-*`, `l3` vectors, r0's l3cg/l3cov) retire, each replaced by a
  program-engine golden where it guards a property we still need. Rename model types.
  Acceptance = "Done means" above, plus an inventory showing zero legacy resolver /
  event-builder calls or surviving implementations (migration-only tooling included) and
  every cross-app context field accounted for.
- **S10 — Compact ids** (spec §8) on the single substrate.

## Decisions (owner)

1. Move, not rewrite, the L2 walker — it becomes part of the program engine
   (`src/program/body/`). Recommended; reviewer concurs.
2. Retire L3-only golden families in S9, replacing those that guard a needed property.
3. One branch per step, merged as each passes.

## Risks

- G6: a single mis-minted durable id moves goldens wholesale; S2b proves id equality on
  the whole corpus first.
- G8: route loss behind a matched site is silent unless route accounting is exact.
- Memory: S2a/S2b should lower peak (one parse, no L3 assembly); S7/S8 under `FULL` may
  raise it. Measure, do not assume.

## Changes from rev 2 (review round 2: AGREE-WITH-CHANGES, all accepted)

1. G15 split: G15a (declaration + analysis-readiness metadata, four body states, separate
   target registry) lands in S2b, before S3/S4; G15b (body analysis) in S7/S8.
2. S7 now replaces the existing dependency products (`build_dep_artifact_l4`,
   `recover_dep_retained`) from program parses, with owning-app body resolution as a
   primitive; S8 is only the Phase C expansion. No hybrid bridge.
3. Demanded-trees profile explicitly deferred, with the discovery/cache contract it must
   meet before it may ever land; `FULL` is the switch's profile.
4. Minors: G6 wording on `RoutineNodeId`; S2a reproduces old file-set and failure
   policy; downstream route selection joins the G8 audit; S9 "zero legacy" wording.

## Changes from rev 1 (review round 1)

1. S2 split into S2a (parse sharing) and S2b (model assembly, with table/object facts
   moved in from S4). 2. G1 lists L3's enrichment passes and the ordering timing.
3. New population/order contract (G5). 4. G6 split into three identities, one-to-many
   mappings, fixtures; physical rows not deferred. 5. G13 corrected: byte columns both
   sides (rev 1 wrongly proposed a UTF-16 conversion), app-qualified keys, DFS site
   links. 6. Adapter completion is its own step with route accounting. 7. G7 is a full
   subscription inventory. 8. d43 needs reverse demand from events to raisers; d45 needs
   a scope decision; S6 split into S7/S8. 9. G9 coverage not from `ProgramReport.
   coverage`; G14 app ledger. 10. G15 body states + ABI retention; artifact builds use
   full population; memory note corrected. 11. Deletion checklist, wider S1 guard,
   nested rebuilds, executable regeneration in "Done means". Harness compares more and
   keeps two baselines.
