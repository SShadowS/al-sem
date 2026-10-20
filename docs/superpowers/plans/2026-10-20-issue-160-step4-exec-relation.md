# #160 step 4: `ExecRelation` — plan

Spec: `docs/superpowers/specs/2026-10-20-issue-160-step4-exec-relation-design.md`
(converged, spec panel round 3, hash `0bcdf34b92da722c`; "spec §n" below). Epic:
`docs/superpowers/specs/2026-10-18-issue-160-analysis-session-design.md` §5.5, §9 row 4.
Assumption probes P1-P38: `.agent/issue-160/ledger.md`.

Seven tasks, one commit each, in this order. Step 4 is byte-neutral: **no golden may move
in any commit**. Counter rows move only in the commit whose code moves them (Task 5).

## Rules every task follows

- Edit files with the Edit/Write tools only. No `sed -i`, `perl -pi`, heredoc or script
  rewrites, including discrimination breaks (break with Edit, revert with Edit).
- Format with `rustfmt <file>` per touched file; never `cargo fmt`.
- No `#[allow(` anywhere. Dead code is deleted, not silenced.
- Protected paths, never edited: `.github/`, `scripts/`, `.claude/`, `CLAUDE.md`,
  `.gitignore`, `Cargo.toml` version fields, `tree-sitter-al/`, `.agent/` (except the
  ledger's discrimination-proof table and Timeline lines the task names).
- Environment: `export TREE_SITTER_AL_PATH=U:/Git/al-call-hierarchy/tree-sitter-al`.
  CDO only through a per-command prefix:
  `CDO_WS=U:/Git/DO-cdo-baseline/Cloud cargo test --profile cdo -p al-sem --lib <filter>`.
  Never export `CDO_WS`.
- Tests are written red first where the behaviour is new; byte pins of today's behaviour
  are written green on the BASE code first (Task 1), as spec §5.2 requires.
- Every new test gets a discrimination proof: break (Edit), run, record the failing
  assertion, revert (Edit), run, record the pass. Assert the break applied (the failure
  must name the assertion the proof targets, not a compile error or another test). Record
  each in the ledger's proof table: test | mutation | fail output | pass output | commit.
- Per-task gate, before the commit: `scripts/ci-steps task-affected` green, then commit;
  the pre-commit golden hook must pass unmodified (never `--no-verify`). Do not pipe a gate
  through `| tail`; redirect to a log and `grep` it.
- Stage only the task's files (`git add <paths>`), never `git add -A`.

---

## Task 1 — pin today's behaviour with the three fixtures, on the base code

Spec §5.2, §5.3, §5.4 (second assertion only). No production code changes.

Files: `src/engine/l4/capability_cone.rs` (its `tests` module only),
`src/engine/l5/test_support.rs` (`#[cfg(test)] pub(crate)`, `l5/mod.rs:158-159`; the
fixture writers live here so Task 2's `exec_relation::tests` reaches them too).

1. Add the shared helper `dependency_half(ws: &Path) -> Vec<CallEdge>` (spec §5.2, S2-3):
   `InputSet::read(ws, &Default::default())`, then
   `program_calls::assemble_and_resolve_cross_app_program(&inputs, "r0", false)`
   (`program_calls.rs:897-901`), then the uncut `resolved.calls.edges` filtered to the
   routines whose `app_guid` (lower-case) differs from `resolved.primary_app`'s, the base's
   own rule (`capability_cone.rs:3562-3569`). Emission order kept.
2. Add `write_fixture_step4(ws)` on `write_fixture_107`'s pattern (`:5189-5226`), exactly
   as spec §5.2 lists it: `SymbolReference.json` with table 18 and codeunit 12
   `Dep Rec Lib` (`Post`, `OnDepPost` with the `IntegrationEvent` attribute JSON,
   `CallTouch`); dependency `src/Lib.al` (`Post` raises `OnDepPost`; `CallTouch` with a
   `temporary` record calls `local procedure Touch(var R)` doing `R.Modify()`);
   `src/Customer.al`; workspace codeunit `Ws Fwd` with `DoPost`, `DoTouch`, and
   subscriber `HandleWs` on `OnDepPost` doing `Rec.Modify(); Commit();`.
3. `step4_dependency_publisher_primary_rule` (spec §5.2): `build_cross_app_base`, then
   `View::new(CrossApp(..))`, `prepare(&digest_plan(default_detectors()))`, read
   `view.det()`. Preconditions from immutable inputs (model rows, model event graph,
   `dependency_half`), then the three literals: (1) `reverse_call_graph[HandleWs]` is one
   `event-dispatch` edge from `OnDepPost`; (2) `HandleWs`'s commit span roots are
   `[OnDepPost]`; (3) no `closed_world_temp_params` entry for `Touch`.
4. `step4_from_order_picks_the_first_caller` (spec §5.3): tempdir with `ws-d17-drift`'s
   `app.json` and `.alpackages` copied and two caller files `A.al`, `B.al`; precondition
   on `base.resolved.calls.edges` (first `A` edge before first `B` edge); run
   `analyze_plan(vec![d17], true)` through the view; literal: one
   `d17/dddddddd-0000-0000-0000-0000000000d17` finding with evidence `routine_id` = `A`'s
   routine and `affectedObjects` = `[A's object]`.
5. `step4_dependency_uncertainty_stays_out_of_primary` (spec §5.4, part 1 of 2): inline
   fixture with dependency `DepCaller` making a to-less call, called from the workspace;
   precondition via `dependency_half`; assert no `base.graph.uncertainty_edges` entry and
   no context `uncertainty_edges_by_from` key is `DepCaller` (a plan holding d1). Task 5
   renames `base.graph` to `base.rel.graph` and adds part 2.
6. Run all three on this (base) code. They must pass. **If any literal disagrees, stop**:
   record the observed value in the ledger Timeline and report to the conductor; the spec's
   reading was wrong and goes back to the panel (spec §5.2). Do not adjust a literal.
7. No discrimination proof in this task: each break targets `ExecRelation::new`, which
   Task 5 records. Timeline line: "Task 1: §5.2/§5.3/§5.4 literals confirmed on base
   `c554b704`".
8. Gate: `cargo test -p al-sem --lib step4_`, then `scripts/ci-steps task-affected`.
   Commit: `#160 step 4: pin the dependency-publisher, from_order and dependency-uncertainty behaviour on the base`.

## Task 2 — commit A: `ExecRelation` over a copied assembler, and the independent oracle

Spec §4.1, §4.2, §5.1 commit A. No consumer is wired yet; no counter moves (nothing in
production calls `new`).

Files: `src/engine/l4/exec_relation.rs` (new), `src/engine/l4/mod.rs` (`pub mod
exec_relation;`).

1. In `exec_relation.rs`: a private `assemble(ws, edges: &[CallEdge], events) ->
   CombinedGraph`, a COPY of `build_combined_graph`'s body (`combined_graph.rs:259-430`,
   minus the counter) and of `build_typed_edges` (`:519-933`) with the private helpers they
   call (`uncertainty_sort_key`, `is_edge_kind_enum`, `is_object_run_kind`,
   `dispatch_kind_to_object_type`, `simple_uncertainty_cs`/`_op`,
   `object_run_target_id_source`, `object_run_target_object`), taking `&[CallEdge]`
   instead of `&ResolvedCalls`. `build_combined_graph` stays untouched. Use the public
   `edge_sort_key` from `combined_graph`.
2. `pub struct ExecRelation { pub graph: Arc<CombinedGraph>, pub scc: Arc<SccResult>, pub
   dependency_uncertainty: Vec<UncertaintyEdge> }`, `pub fn new(ws, primary: &[CallEdge],
   dependency: &[CallEdge], events: &EventGraph)` per spec §4.2 steps 1-5 (counter
   `build.combined_graph` once, span `rel.build`, the dependency half-build only when
   non-empty with an empty event graph, `graph_scc`, census buckets `rel.graph`,
   `rel.scc`, `rel.dependency_uncertainty` when `census::enabled()`), `pub fn cone()`,
   `pub fn primary()`, with the spec §4.1 docs (M1/M2, one-directional guarantee).
   Everything `pub`, so the unwired type raises no dead-code warning.
3. Red first: `new` returns an empty relation; write in `exec_relation::tests`:
   - `legacy_relation(ws, all_calls, events, dep_ids)` (`#[cfg(test)]`): today's
     `capability_cone.rs:3571-3617` plus `:3588`, calling the untouched
     `build_combined_graph`, returning the dependency half-build's `uncertainty_edges`.
   - `relation_equals_the_legacy_base_per_field` (spec §5.1): every app dir at any depth
     under `tests/r0-corpus`, `tests/fixtures`, `tests/r3a4-fixtures`, `tests/r3a5-fixtures`,
     `tests/r2-5b-fixtures`, plus the §5.2 inline fixture (Task 1's
     `test_support::write_fixture_step4`);
     single-app arm with `dep_ids = ∅` against `new(ws, &calls.edges, &[], ..)`, cross-app
     arm with the base's split. Per field, separately: `nodes`, `edges_by_from`,
     `edges_from_order`, `uncertainty_edges`, `typed_edges` (Vec), SCC,
     `dependency_uncertainty`. A dir with no model fails. Preconditions from inputs: one
     case with a non-empty dependency half AND an event-dispatch edge; print (not require)
     the count of cases with dependency uncertainty.
   - `relation_equals_the_legacy_base_on_cdo`: the same on CDO, both models, gated by
     `crate::cdo::cdo_ws_or_enforce()`.
   - `isolated_dispatch_precondition` inside the parity test or beside it (spec §5.1,
     S2-1): on `tests/r0-corpus/ws-txn-d47-event-neg-isolated`'s model event graph, an
     `EventSymbol` with `isolated == Some(true)` and an `EventEdge` on it; fails, never
     skips, if absent.
4. Green: implement `new`. Run the fixtures test, then the CDO half:
   `CDO_WS=U:/Git/DO-cdo-baseline/Cloud cargo test --profile cdo -p al-sem --lib relation_equals_the_legacy_base_on_cdo`.
5. Proofs (on this commit, the independent one): in the COPIED `assemble`, (i) sort each
   `edges_by_from` list by `to` instead of `edge_sort_key` → `edges_by_from` fails;
   (ii) set every `DispatchMeta.isolated` to `false` → `edges_by_from` fails on
   `ws-txn-d47-event-neg-isolated`. Each reverted, green. Record both.
6. Gate: `cargo test -p al-sem --lib exec_relation`, then `scripts/ci-steps task-affected`.
   Commit: `#160 step 4: ExecRelation over a copied assembler, with the independent parity oracle (commit A)`.

## Task 3 — commit B: one assembler

Spec §4.2 step 2, §5.1 commit B.

Files: `src/engine/l4/combined_graph.rs`, `src/engine/l4/exec_relation.rs`.

1. Text-identity check BEFORE deleting the copy, read-only: extract the old
   `build_combined_graph` body (after the counter line) plus `build_typed_edges` and the
   helpers from `combined_graph.rs`, and the copy from `exec_relation.rs`, into the
   scratchpad, and `diff` them. The only allowed differences: `resolved.edges` ↔ `edges`
   and the parameter list. Paste the diff output (or "no other difference") into the
   ledger Timeline.
2. In `combined_graph.rs`: `pub(crate) fn assemble(ws, edges: &[CallEdge], events)` holds
   the body; `build_typed_edges` takes `&[CallEdge]`; `build_combined_graph` = the counter
   + `assemble(workspace, &resolved.edges, event_graph)`.
3. In `exec_relation.rs`: delete the copy and its helpers; `new` calls
   `combined_graph::assemble`.
4. Proofs (the composition discriminations, spec §5.1 (a)-(d), against `new`): (a)
   dependency typed edges extended first → `typed_edges` fails; (b) `events` passed to the
   dependency half → `typed_edges` fails on the §5.2 case; (c) dependency edges put into
   the primary half → `edges_by_from` fails; (d) dependency uncertainty appended to
   `uncertainty_edges` → `uncertainty_edges` fails. Each reverted, green.
5. Gate: `cargo test -p al-sem --lib exec_relation combined_graph`, then
   `scripts/ci-steps task-affected`. Run the CDO half again with the prefix.
   Commit: `#160 step 4: build_combined_graph delegates to the one assembler (commit B)`.

## Task 4 — the cone takes the Cone payload

Spec §4.5 (signature item), M2. Mechanical, byte-neutral, no counter moves.

Files: `src/engine/l4/capability_cone.rs`, `src/engine/l5/detector_context.rs`,
`src/engine/l4/cone_derived.rs`.

1. `compose_cone_over_graph(typed: &[TypedEdge], nodes, ..)` and
   `build_typed_edge_graph(typed: &[TypedEdge], nodes)`. Update every call site with one
   argument (`&graph.typed_edges`): `detector_context.rs:1230`, `capability_cone.rs:2832`,
   `:3847`, `:4033` (`project_r3a3`), `:4281`, `:4404` (`compute_r3a3_real_matrix`), and
   the 10 calls in `cone_derived.rs` (`:1070` … `:1646`). Doc of
   `compose_cone_over_graph`: the parameter is `Transfer::Cone`.
2. No new test (a compile-time guarantee; the goldens and lib tests are the check).
3. Gate: `scripts/ci-steps task-affected`.
   Commit: `#160 step 4: the cone reads only the typed edges (Transfer::Cone)`.

## Task 5 — every consumer reads the relation; counters, renames, rewritten tests

Spec §4.3, §4.4, §4.5, §4.8, §4.9, §5.2-§5.6. The one commit where counters move.

Files: `src/engine/l4/capability_cone.rs`, `src/engine/l4/analysis.rs`,
`src/engine/l5/detector_context.rs`, `src/engine/session.rs`,
`src/engine/gate/policy/pipeline.rs`, `src/engine/l4/effect_query_cli.rs` (if
`into_query_parts`' caller needs it), `src/engine/l4/summary_runner.rs` (test),
`src/engine/gate/events.rs` (only if it names `Cell::Graph`),
`tests/cli/cli_build_counters.rs`, `tests/cli/cli_digest_query_dependencies.rs` (doc).

1. Red first (new assertions on the new API):
   - add part 2 of `step4_dependency_uncertainty_stays_out_of_primary`:
     `base.rel.dependency_uncertainty` holds exactly one edge `from = DepCaller` of the
     kind its resolution maps to (`combined_graph.rs:335-361`); rename part 1's
     `base.graph` read to `base.rel.primary()`;
   - edit `cli_build_counters.rs` to spec §4.9: `bcg` 1 in `analyze_cross_app_default`,
     2 in `analyze_cross_app_d47`, 2 in `digest_cross_app`, 1 in `EVENTS_CROSS_ROW`,
     1 in `policy_check_cross_app`, 1 in `QUERY_CROSS_ROW`; update the section header
     comment to "after step 4".
   Both red on the Task 4 code.
2. Green, per spec:
   - `build_cross_app_base`: split `x.resolved.calls.edges` into primary/dependency
     `Vec<CallEdge>`; `ExecRelation::new(ws, &primary, &dependency, &x.resolved.events.graph)`;
     leaf/fixed split reads `rel.graph.edges_by_from`; cut model calls from `primary`
     (upgraded bindings, diagnostics, external targets as today; no clone for the
     dependency half). `R3a5CrossAppBase.rel: Arc<ExecRelation>` replaces `graph`,
     `combined_scc`, `event_graph`, `nodes`. Census: drop those four `base.*` buckets.
     Spans: `crossapp.base_graph` → `crossapp.field_index` (around the field index only),
     `crossapp.base_artifacts_facts` → `crossapp.base_facts`.
   - `build_l4(model, rel: &ExecRelation, req, cross)`; `L4Request { extra_diagnostics }`;
     `L4Analysis.scc = Arc::clone(&rel.scc)`; delete the Tarjan, its span
     `context.core_scc_tarjan`, `census_l4`'s `computed_scc` and the `l4.scc` branch.
   - `build_detector_artifacts(model, demanded, rel: &Arc<ExecRelation>, l4, cross)`:
     `DetectorArtifacts.graph = Arc::clone(&rel.graph)`, cone over `rel.cone()`, then
     `build_l4(model, rel, ..)` (cone first, then solve, then harvest).
   - Delete `context_graph` and its span; the shims build
     `ExecRelation::new(ws, &resolved.calls.edges, &[], &resolved.events.graph)` single-app
     or take `Arc::clone(&base.rel)`.
   - `View`: `graph` cell → `rel: Option<Arc<ExecRelation>>`; `Cell::Graph` → `Cell::Rel`
     (also in `Cell::ALL`, `release`, `release_all_but` callers); `View::graph()` →
     `View::rel()`; `prepare` fills it single-app with `ExecRelation::new(..)` and
     cross-app with `Arc::clone(&base.rel)`; no `scc` plumbing left. `into_query_parts`
     returns `Arc::clone(&rel.graph)`.
   - Policy RAW build passes `view.rel()`; `release_all_but(&[Cell::Rel])`,
     `release(Cell::Rel)`.
   - `project_r3a5_cross_app`: `base.rel.graph`, `base.rel.scc`,
     `base.resolved.events.graph`, node ids from `base.ws_routines()`, cone over
     `base.rel.cone()`.
   - `issue160_full_graph_timings`: `(*base.rel.graph).clone()`.
   - Doc comments re-stated (spec §4.5 list): `session.rs:4`, `:28`, `:312-313`;
     `detector_context.rs:1060-1062`, `:1107-1109`; `capability_cone.rs:3442-3448`,
     `:3454`, `:5045-5047`; `analysis.rs:26-27`; the legacy-cut doc line on
     `build_r3a3_source_only_base` (M9).
3. Rewritten tests (spec §5.6): `retained_census_counts_the_single_app_scc_once` (one
   `bucket=rel.scc`, zero `l4.scc`); `retained_census_counts_the_cross_app_graph_and_scc_on_the_base`
   → `…_on_the_relation` (one `rel.graph`, one `rel.scc`; zero `base.graph`,
   `base.combined_scc`, `context.graph`, `l4.scc`); `views_release_their_cells_before_the_next_build`
   (`rel:Graph` → `rel:Rel`, doc `Cell::Rel`); `cross_app_shim_reuses_the_base_scc`
   (`ptr_eq(&l4.scc, &base.rel.scc)`); `dependency_publisher_roles_stay_unknown`
   (signature); `core_summaries_without_an_l4_request_panics` (builds a relation);
   `summary_runner` test at `:2584-2590`; `session::tests` (`Cell::Graph` at `:743`).
   `cross_app_query_drops_the_base` keeps `(1, 1)`; only its doc changes.
4. Gate inner loop: `cargo test -p al-sem --lib step4_ exec_relation session
   capability_cone detector_context analysis`, `cargo test --test cli cli_build_counters::`,
   `cargo test --test cli cli_digest_query_dependencies::`.
5. Proofs (all against the final code):
   - §5.2 "Primary callers only": in `new`, drop `event-dispatch` edges whose publisher is
     a dependency routine → (1) and (2) fail (record the observed root `HandleWs`); (3)
     stays green (the stated limit). Revert, green.
   - §5.2 "every caller": in `new`, assemble Primary over `primary ++ dependency` → (2)
     fails (roots `[DoPost]`), (3) fails (`Touch` proven). Revert, green.
   - §5.3: reverse `edges_from_order` in `new` → precondition holds, the finding's sample is
     `B` (record its routine id). Revert, green.
   - §5.4: break (d) → part 1 fails; drop the `dependency_uncertainty` assignment → part 2
     fails. Revert, green.
   - Counters (§5.5): (a) build the dependency half through the counted
     `build_combined_graph` → `analyze_cross_app_default` reads `bcg` 2; (b) a Tarjan in
     `build_l4` ignoring `rel.scc` → `analyze_single_app` reads `scc` 3. Revert, green.
   - `cross_app_commit_span_sees_dependency_writes`: skip the dependency typed-edge extend
     in `new` → `["span-has-no-known-writes"]`; revert, green; re-state its doc
     (`cli_digest_query_dependencies.rs:466-474`) with this seam and date.
6. Gate: `scripts/ci-steps task-affected` (expect it to run most of the suite; the change
   is wide), then the CDO half with the prefix.
   Commit: `#160 step 4: every consumer reads the one ExecRelation; counters, renames`.

## Task 6 — documents: epic amendments, child 0-2 table, CHANGELOG

Gate-inert prose (`CHANGELOG.md`, `docs/**/*.md`).

Files: `docs/superpowers/specs/2026-10-18-issue-160-analysis-session-design.md`,
`docs/superpowers/specs/2026-10-18-issue-160-steps-0-2-design.md`, `CHANGELOG.md`.

1. Epic: one short "Amended at step 4 (child spec `2026-10-20-…-step4-exec-relation-design.md` §9):"
   note where each lands:
   - §5.5 (struct and field table): M1 (carrier), M2 (accessors, one-directional),
     M3 (Cone pair deferred, tradeoff incl. policy's duplicate), M5 (uncertainty split),
     M6 (no events/bindings/targets copy), M7 (no `role`), M8 (`dependency_calls` at step
     6); Decision 18 gets "(placement in time amended at step 4, M8)";
   - §9 row 4: M4 (counter rule) and M9 (`LegacyCut` a doc name); the row's counter text
     points at the child spec §4.9 rule;
   - §5.9 (`pub rel`): one line that at step 4 `rel` is a `prepare`-filled cell (spec
     §4.4) and the single-app SCC lifetime note.
2. Child 0-2 §4.4.8: add "**After step 4**" with spec §4.9's table and rule, moved cells
   bold, and one line on the trace/census renames.
3. `CHANGELOG.md` `[Unreleased]` → `### Changed`: "#160 step 4" entry: what (one
   `ExecRelation` per view; who reads which payload), byte identity (no golden, CDO
   detector baseline and histogram unchanged — numbers from Task 7), counters (`bcg` per
   row), dependency uncertainty kept, spans/census/cell renames, single-app SCC lifetime
   and its bound, tests rewritten, the two-commit oracle. Leave measured values as
   `<Task 7>` placeholders for Task 7 to fill.
4. Gate: `scripts/ci-steps task-affected` (docs-only; fast).
   Commit: `#160 step 4: epic amendments M1-M9, child 0-2 after-step-4 table, CHANGELOG`.

## Task 7 — measurement and numbers file

Spec §7. The implementer prepares; **the conductor runs the CDO A/B peak bench** through
the supervisor (`agentflow run --name bench`). No cargo build may run during a bench.

Files: `docs/measurements/2026-10-20-issue-160-step4.md` (new), `CHANGELOG.md` (fill the
placeholders).

1. Binaries (`release-fast`; never `cdo` for timings or memory):
   - parent: a scratch `git worktree add <scratch>/base c554b704` with
     `TREE_SITTER_AL_PATH` set, `cargo build --profile release-fast --bin alsem --bin aldump`,
     copy both `.exe` to `<scratchpad>/160-step4/before/`;
   - head: the same from the branch HEAD into `<scratchpad>/160-step4/after/`.
2. Findings (implementer runs; ~1 min each, in the background):
   `python U:/Git/al-call-hierarchy/.claude/skills/cdo-measure/scripts/measure.py run --bin <before> --label before --out <m>`,
   the same for `after`, then `measure.py comp --out <m>`. Required: no detector changed
   at `--scope primary` or `--scope all`; fingerprints added 0, removed 0; both `meta.json`
   name `bc3ccb18` with the same dirty count. A non-zero delta fails the step: stop and
   report.
3. Histogram: `<bin>/aldump.exe --program-call-graph-stats U:/Git/DO-cdo-baseline/Cloud`
   per side, SHA-256 of the JSON equal.
4. Census: `ALSEM_RETAINED_CENSUS=1 <bin>/alsem.exe analyze U:/Git/DO-cdo-baseline/Cloud --format sarif`
   and the same with `--single-app`, per side; list every changed bucket with its bytes
   (spec §4.8).
5. Peak bench (prepare; the conductor runs): write into the numbers file the exact
   command, e.g.
   `python scripts/bench_ab.py run --out <dir> --a parent=<before>/alsem.exe --b head=<after>/alsem.exe --rounds 5 --cmd as='analyze U:/Git/DO-cdo-baseline/Cloud --single-app --format sarif' --cmd ps='policy check U:/Git/DO-cdo-baseline/Cloud --single-app --policy U:/Git/al-sem-issue-160-a7/tests/fixtures/cli-c-policy/ws-policy-custom/al-sem.policy.yaml --format json --deterministic' --cmd es='events fanout U:/Git/DO-cdo-baseline/Cloud --single-app --format json --deterministic' --cmd ax='analyze U:/Git/DO-cdo-baseline/Cloud --format sarif' --cmd dx='digest U:/Git/DO-cdo-baseline/Cloud --diff <the diff file docs/measurements/2026-10-19-issue-160-step2.md used> --deterministic'`
   (use the owner's CDO policy instead of the fixture policy if one exists; say which).
   Bounds table for `as`, `ps`, `es`: process peak and post-attach peak, head median ≤
   parent median + 3 % of that command's parent median (bands written from the parent runs
   before reading the head); stdout hashes equal on both sides. `ax`, `dx` reported only.
   After the conductor's run: `bench_ab.py summary <dir>/runs.jsonl` into the numbers file.
   A miss: fail the step, report; the fallback (separate SCC cell, spec §4.4) needs a
   spec change, not a silent edit.
6. Numbers file: protocol, both SHAs, findings comparison, histogram SHAs, census table,
   bench table with the bounds verdict. Fill the CHANGELOG placeholders.
7. Gate: `scripts/ci-steps task-affected` (docs-only).
   Commit: `#160 step 4: numbers file`.

---

## Acceptance map (spec §6)

A4.1 Tasks 2, 4 · A4.2 Tasks 2, 5 · A4.3 Task 5 · A4.4 Task 5 · A4.5 Tasks 4, 5 (no
snapshot behaviour change) · A4.6 Tasks 1, 5 · A4.7 every task (goldens), Task 7
(histogram) · A4.8 Task 7 · A4.9 Tasks 1, 5 · A4.10 Tasks 1, 5 · A4.11 Tasks 2, 3, 5 (CDO
half) and the branch gate (`scripts/cdo-gate`) · A4.12 Tasks 2, 3, 5.
