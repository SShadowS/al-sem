# Issue #159 ledger

## Claim

- run: `20261023-090537-f1612b`, attempt 1
- body_hash: `a28d6c205c8805ca4d29d6fad5b9c51180e9b160b84d834d98c68081ac44385d`
- base: `84ba2d4b`
- worktree: `/work/al-sem-issue-159-a1`, branch `issue/159-al-call-hierarchy-analyze-forma-a1`
- operator: CentralGauge demo 2026-10-27; release v2.0.1 by 2026-10-24. Keep it tight.
- reviewers: gpt-6.1-sol (astra) and Opus (flash); `--substitute astra=sol --substitute flash=opus`

## Timeline

- 2026-10-23 09:05 claim (run above), worktree at base `84ba2d4b`.
- 2026-10-23 09:10 classified bounded; probes P1-P13 run (dev build of `al-call-hierarchy`, probe workspace under `.agent/runs/20261023-090537-f1612b/probe-ws`).
- 2026-10-23 09:20 `## Design` v1 written. Caps: pi_calls 0, spec panels 0.
- 2026-10-23 spec panel round 1 (sol: 4 important, 2 minor, S1/S3/S4 rejected; opus: 4 important, 6 minor). Conductor: Q1 text skips `unused_procedure`; Q2 triggers `access: "public"` + `is_trigger`. Probes P14-P20 run; `## Design` v2 (anchor_hash `design-r1`); register R1-01..R1-14 written. Caps: spec panels 1.
- 2026-10-23 conductor: Q3 keep `embedded`; Q4 CentralGauge uses an external 201-package symbol store → probe P21.
- 2026-10-23 spec panel round 2: opus accepted 14 + N1 (important), N2 (minor); sol accepted 12, re-raised R1-13/R1-14 (minor). Design fixed (anchor_hash `design-r2`): per-field null semantics, routine_id CHANGELOG wording, test 6 relative path + parsed-Value compare. Register R2-01/R2-02 added. Caps: spec panels 2.
- 2026-10-23 acceptance tests written (uncommitted): see `## Acceptance tests`. 7/7 compile and fail on base for the stated reason.

## Acceptance tests

Files (uncommitted): `tests/cli/cli_analyze_call_hierarchy_json.rs` (+ `mod` line in
`tests/cli/main.rs`); fixtures `tests/fixtures/analyze-json/` (8 `.al` files + app.json),
`tests/fixtures/analyze-json-noapp/Solo.Codeunit.al`; frozen base outputs
`tests/fixtures/analyze-json-expected/legacy.{json,txt,csv}`, minted with the base
`target/debug/al-call-hierarchy` (worktree source == `84ba2d4b`, `git status` showed only
untracked files) from cwd = worktree root, `--project tests/fixtures/analyze-json`; two
runs gave identical JSON (`cmp`).

Fixture checks on base, before writing tests: the LSP (stdio driver) flags ONLY
`Probe Util.NeverCalled` (line 34); `alsem fingerprint --inventory-only` lists 22 routines,
including `TableExtension 50106 OnValidate (member Note)` and `Page 50107 OnAction` ×2
(members First/Second). The base JSON has one legacy finding (NeverCalled
`too_many_parameters`), so the "appended after" check has something to come after.

Run on base: `cargo nextest run --test cli cli_analyze_call_hierarchy_json::` → 7 run,
0 passed, 7 failed (`/tmp/i159-acc.log`):

| test | fails on base because |
|------|-----------------------|
| fan_in_and_fan_out_count_distinct | `row "Probe Util"."Target" has no fan_in field` |
| unused_procedure_findings_equal_the_lsp_diagnostics | `unused_procedure findings: []` (left `{}`, right `{NeverCalled}`) |
| path_is_workspace_relative_posix | no `path` field |
| object_id_access_and_kind_flags | `row "Probe Util"."Target" has no object_id field` |
| routine_id_equals_alsem_stable_routine_id | no `routine_id` field |
| legacy_fields_text_and_csv_byte_identical_to_base | parsed legacy compare PASSES on base; fails at `row key order` (the 9 legacy keys only). The text/CSV byte checks run after it and match by construction on base |
| no_app_json_degrades_to_null | `row "Probe Solo"."Lonely" has no fan_in field` |

Test 8 (`join_miss_gives_null_not_zero`, a lib unit test in `analysis.rs`) is NOT written
yet: it calls the new `analysis::analyze_project` facade, which does not exist on base, so
it cannot compile there. It is written with the implementation, red first against a stub.

## Classification

**Bounded.** Every datum the Acceptance asks for already exists and is already computed by a
shared, tested function: the resolved call graph (`LspSnapshot`), the LSP's unused rule
(`diagnostics::is_unused_procedure` + `lens::effective_incoming_count`), the workspace-relative
path (`source_text::AlFile::rel_posix`, which `run_analysis` currently throws away), the
routine's kind/access/attributes (`RoutineDecl`), and alsem's stable routine id
(`ids::to_stable_routine_id_from_parts`, built in `l2_workspace::build_proutine`). The change
joins them in one library function and adds fields. No engine, resolver, id schema or golden
family changes. One CLI mode, one output struct, additive only.

## Probes

| probe | result | evidence |
|-------|--------|----------|
| P1 current `--analyze --format json` shape | `metrics[]` rows carry only `object_type, object_name, procedure_name, file, line, complexity, line_count, parameter_count, quality_score`; `findings[]` has only `high_complexity/long_method/too_many_parameters`; `file` is the base name | `.agent/runs/20261023-090537-f1612b/probe-json.out`: `src/A/Util.Codeunit.al` and `src/B/Util.Codeunit.al` both print `"file": "Util.Codeunit.al"`; the two `Over` overloads differ only by `line` (37, 41) |
| P2 where the output is built | `src/main.rs::run_analysis` (walks with `source_text::discover_al_files(.., NestedApps::Walk)`, then `.map(\|f\| f.abs_path)` drops `rel_posix`) + `extract_metrics_ir` (own `al_syntax::parse`, per object per routine); structs in `src/analysis.rs` (`ProcedureMetrics`, `Finding`, `AnalysisSummary`, `AnalysisResult`) | `src/main.rs:185-300`, `src/analysis.rs:1-45` |
| P3 `--analyze` builds NO call graph today; `run_analysis` takes no `DependencySource` | confirmed | `src/main.rs:123-125, 185` |
| P4 the LSP call graph is buildable from the CLI | `LspSnapshot::build_full_with(project, dependency_source)` already used by `report_index_stats`; on the probe workspace: 12 definitions (= the 12 metric rows, triggers and the publisher included), 7 call sites (= 3 + 2 + 1 + 1 written in the fixture) | `src/main.rs:162-182`; probe run log |
| P5 snapshot needs `app.json`; `--analyze` does not | copy of one `.al` file without `app.json`: `--analyze` prints 11 rows, `--project` alone fails "Failed to build the program snapshot … (a readable app.json at its root)" | `/tmp/i159-noapp` probe |
| P6 join key metric row → `DeclEntry` | snapshot `virtual_path` == `AlFile::rel_posix` (doc: "Root-relative, `/`-separated, case kept (… the snapshot's `virtual_path`)"); both walks are `NestedApps::Walk`; `DeclEntry.origin` is copied byte-for-byte from `RoutineDecl.origin` (`lens::find_routine_by_origin` already correlates on `origin.byte.start`) | `src/source_text.rs:54-59`, `src/lsp/lens.rs:141-156` |
| P7 how the LSP decides "unused" | `diagnostics::compute_file`: `cfg.unused_procedures && is_unused_procedure(decl, routine, effective_incoming_count(snap, &decl.id))`; rules R1 (procedures only), R6 (interface sigs never), R4 (Integration/Business publishers never), R3 (test/handler attributes never), else `effective_incoming_count == 0` where that = `snap.incoming_count(id)` (call refs + EventFlow refs to a subscriber) + `snap.publisher_fanout(id)`. An `[EventSubscriber]` is "used" only if its EventFlow edge resolves (R2) | `src/lsp/diagnostics.rs:20-57, 234-255, 300-347`; `src/lsp/lens.rs:185-189`; existing test `unused_rule_r2_event_subscriber_wired_to_real_publisher_not_flagged` |
| P8 `is_unused_procedure` is module-private | `fn is_unused_procedure` (no `pub`); `effective_incoming_count` is `pub(crate)` | `src/lsp/diagnostics.rs:300`, `src/lsp/lens.rs:185` |
| P9 incoming refs are per call SITE, not per caller | `incoming_count` = number of `EdgeRef`s; `handlers::incoming` groups them by `edge.from` to get distinct callers. So `fan_in` must dedupe on `from` (CallerA calls Target twice = 1 caller) | `src/lsp/snapshot.rs:903-916`, `src/lsp/handlers.rs:156-175` |
| P10 outgoing selection | `handlers::outgoing` = `edges_by_file[decl.virtual_path]` filtered `from == id` + `ws_event_edges` filtered `from == id`; targets are only `LspTarget::Routine`/`LspTarget::Abi` (builtin/unresolved are never stored); a conditional/ambiguous site carries several candidate targets | `src/lsp/handlers.rs:222-275`, `src/lsp/snapshot.rs:144-168` |
| P11 alsem's routine identity | two exist: internal `routineId` (`r0/<sha256>`, hashes the routine KIND too, so adding `[EventSubscriber]` moves it) and `stableRoutineId` = `{appGuid}:{ObjectType}:{N}#{normalizedSignatureHash}` (+ member discriminator for member triggers) — the key `alsem diff`/snapshot join on and that finding fingerprints substitute to. Inputs are pure IR: root app guid, `ir_object_type`, object number, `ir_parameter_symbols` + return type → `normalized_signature_hash`, `ir_enclosing_member`. Overloads differ in param types → distinct hash | `src/engine/ids.rs:206-216, 300-336`; `src/program/body/l2_workspace.rs:96-125, 266-273`; `src/engine/snapshot.rs:40, 200` |
| P12 tests/goldens covering this output | NONE. No test runs `al-call-hierarchy` or `run_analysis`/`extract_metrics_ir`; `analysis.rs` unit tests cover only the score/findings/summary helpers (11 `ProcedureMetrics { .. }` literals across src). No golden dir holds `--analyze` output. One side effect: `program_resolve_harness::adapter_loses_nothing_cross_app_on_the_fixtures` walks every `tests/fixtures/*` with an `app.json` to depth 3, so a new fixture there is also checked by it | `grep -rln -- --analyze tests/ scripts/` (empty); `grep -rn "ProcedureMetrics {" src \| wc -l` = 11; `tests/program_resolve_harness.rs:10660-10697` |
| P13 text/CSV printers | CSV prints only metric columns (not findings); text prints EVERY `findings[]` entry under FINDINGS | `src/main.rs:330-437` |
| P14 crate boundary (sol 1) | CONFIRMED. `main.rs` is the binary and reaches the library through `pub use al_sem::{analysis, lsp, …}`; `pub(crate)` items of `al_sem` are invisible to it. `analysis` is a library module | `src/main.rs:17-20`, `src/lib.rs:19` |
| P15 the panel drops callers it cannot locate (opus I1) | CONFIRMED. `handlers::incoming` groups by `edge.from`, then `decl_and_line_table(&caller_id) else { continue }` drops a caller with no workspace decl and no embedded source (a symbols-only dependency publisher) | `src/lsp/handlers.rs:165-185` |
| P16 LSP's real unused verdicts on the probe workspace (stdio driver `lsp_probe.py`, open + codeLens to build the lazy root) | flagged: CallerA, CallerB, NeverCalled, both `Over` overloads. NOT flagged: Target, One, Two, Three (called), `OnSomething` (IntegrationEvent, R4), `OnRun` (trigger, R1), `HandleSomething` (subscriber wired to a workspace publisher, R2) | `.agent/runs/20261023-090537-f1612b/lsp_probe.py` output |
| P17 default dependency source and Base App subscribers (opus I4) | `--dependency-source` defaults to `embedded` (env `AL_SEM_DEPENDENCY_SOURCE`), the LSP's default; its help says "Ignored with --analyze". On a workspace with Microsoft System/Base/Application `.app`s in `.alpackages`, a subscriber to `Codeunit::"Sales-Post".OnBeforePostSalesDoc` is NOT flagged under `embedded` AND under `symbols`; a misspelled event name IS flagged under both. Same workspace with NO `.alpackages`: the Base App subscriber IS flagged `unused` | `src/main.rs:69-80`; `.agent/runs/20261023-090537-f1612b/cg-ws`, `/tmp/i159-nopkg` probes |
| P18 cost of the graph (sol 2, F4), release-fast, 22 cores, `cg-ws` = 3 small files + Microsoft System/System Application/Business Foundation/Base Application/Application `.app`s (a CentralGauge-sized solution) | today `--analyze --format json`: 0.00 s, 14 MB peak RSS. Snapshot build alone (`--project` index mode, the same `build_full_with`): `embedded` 2.36-2.47 s, 555-571 MB; `symbols` 1.13-1.19 s, 174-175 MB. 3 runs each; a fresh `HOME` (cold state dir) gave the same (2.43-2.70 s / 563-566 MB; 1.09-1.13 s / 174 MB). So JSON `--analyze` grows to about +2.4 s / +550 MB (embedded) or +1.2 s / +160 MB (symbols) | `.agent/runs/20261023-090537-f1612b/measure.py` output |
| P19 alsem's per-routine `stableRoutineId` is observable from the CLI | `alsem fingerprint <ws> --inventory-only --format json --deterministic` lists every routine as `{objectType, objectNumber, routineName, stableRoutineId}` (+ `enclosingMember` for member triggers). On the probe: `11111111-…:Codeunit:50100#0a4a8b…` for Target; the two `Over` get `#92dfea…` and `#f730fa…`; Interface/ControlAddIn members are not listed | `.agent/runs/20261023-090537-f1612b/inventory.json`; `src/engine/l5/snapshot_full.rs:1295-1330` |
| P20 a second, pre-existing composition of the stable id | `engine::snapshot::snapshot_workspace` re-derives `stable_routine_id` with its own `params_ir` (R0 identity snapshot), separate from `l2_workspace::build_proutine`. Not touched by this issue (out of scope; recorded as a discovery) | `src/engine/snapshot.rs:180-215` |

| P21 how `al-call-hierarchy` finds dependency `.app`s (conductor Q4) | No flag and no env var names a symbol directory. `AL_SEM_DEPENDENCY_SOURCE` only picks `embedded`/`symbols`. The loader (`dependencies::discover_app_files` → `find_all_alpackages_folders` → `package_cache_scope`) reads `<ws>/.alpackages`, then each ancestor's `.alpackages`, stopping after an ancestor that holds `.git`, after 8 ancestors, or at once when `<ws>` itself holds `.git`. A SYMLINK works: `/tmp/i159-link/.alpackages -> cg-ws/.alpackages` loaded 111204 dependency definitions, and the Base App subscriber was no longer flagged | `src/dependencies.rs:357-410, 451-481`; `/tmp/i159-link` probe |

No assumption of the issue was falsified. P5, P13, P15, P17 and P18 are constraints the design
handles.

## Design

(v2, anchor_hash `design-r1`; supersedes v1.)

**Shape.** `ProcedureMetrics` gains, appended after `quality_score` so existing keys keep
name, value and order: `path: String`, `object_id: Option<i64>`, `access: String`,
`is_event_subscriber: bool`, `is_trigger: bool`, `routine_id: Option<String>`,
`fan_in: Option<usize>`, `fan_out: Option<usize>`. `findings[]` gains category
`unused_procedure`, severity `info`, appended AFTER every existing finding (existing entries
keep their array positions). `summary` is unchanged (it counts only `critical`/`warning`).

**Where the code lives (sol 1).** The whole analysis moves into the library behind one public
facade, `al_sem::analysis::analyze_project(project, graph: Option<DependencySource>, cfg)
-> anyhow::Result<AnalysisResult>`. `extract_metrics_ir` and `object_kind_label` move from
`main.rs` into `analysis.rs` unchanged. `main.rs::run_analysis` keeps only argument plumbing
and the three printers. Every rule helper stays crate-private and shared:
`diagnostics::is_unused_procedure` becomes `pub(crate)`; `lens::effective_incoming_count`
is already `pub(crate)`; the new `handlers::incoming_callers` and the stable-id helpers are
`pub(crate)`.

**Graph only for JSON (sol 2).** Text and CSV print none of the new data, so `run_analysis`
passes `graph: None` for them and they run exactly today's code path (no snapshot, no
cost). JSON passes `Some(args.dependency_source)`: the flag is now honoured for
`--analyze --format json`, default `embedded` (the LSP's default, so `--analyze` sees the
graph the editor sees). The `--dependency-source` help text changes from "Ignored with
--analyze" to "With --analyze, used only by --format json (the call-graph fields)".

**One source per file (sol 3).** With a snapshot, rows come from the snapshot's own parse:
for each `AlFile` in `discover_al_files` order (today's order), take
`snap.parsed[rel_posix]` (`file: Arc<AlFile>` + `text`) and walk ITS objects/routines;
the join to `snap.decls_by_file[rel_posix]` is then on `origin.byte.start` within the same
parse, so it cannot mix generations. A file the snapshot does not hold (should not occur,
P6), and every file when there is no snapshot, is read and parsed as today, with graph
fields `null`. The legacy fields are therefore computed by the same functions on text read
by the same decoder (`read_al_source`); a test pins them byte-for-byte against base.

**fan_in (opus I1, M6).** `fan_in` = the number of distinct `edge.from` over
`snap.incoming(&decl.id)` — distinct callers in the resolved graph, event publishers
included as callers of their wired subscribers. One helper,
`handlers::incoming_callers(snap, id) -> HashMap<RoutineNodeId, (bool, Vec<&LspEdge>)>`
(the grouping loop now inline in `handlers::incoming`), which `handlers::incoming` is
rewritten to call. The panel then additionally hides a caller it cannot locate (P15: a
symbols-only dependency publisher), so `fan_in` can exceed the panel's row count by those
callers. This agrees with the unused rule for subscribers (fan_in > 0 ⇒ not unused). It
does not count a publisher's subscribers (those are its callees), so a subscribed but never
raised publisher has `fan_in: 0` and is NOT unused (the unused rule adds
`publisher_fanout`). Both documented in CHANGELOG.

**fan_out (sol 2, opus M2).** `fan_out` = the number of distinct `LspTarget`s
(`Routine(id)` or `Abi(key)`) over the routine's outgoing edges: its call edges
(`Call`/`Run`/`ImplicitTrigger`) and, for a publisher, its event links (subscribers). It
counts distinct TARGETS, not the panel's rows (the panel emits one row per site and route).
Every candidate of a conditional/ambiguous site counts (an interface call with 3
implementations adds 3); builtin and unresolved routes are never stored, so `Rec.Modify()`
adds 0. Computed in ONE pass per analysis, not per routine: `handlers::outgoing_targets_by_caller(snap)
-> HashMap<&RoutineNodeId, HashSet<&LspTarget>>` walks every `edges_by_file` bucket once and
`ws_event_edges` once (cost O(edges), no per-routine scan of the event edges). The panel's
per-routine `outgoing` keeps its own selection; the two are tied by a contract test (below)
on every fixture routine.

**unused.** `diagnostics::is_unused_procedure(decl, routine, effective_incoming_count(snap,
&decl.id))` under `cfg.unused_procedures` — the same three inputs `compute_file` passes, the
same function. Finding: `location` = `file:line` (existing base-name convention),
`procedure` = `Object.Proc`, `description` = the LSP message
`Procedure '<Object>.<Proc>' is never called`. Consequence (P17), stated in CHANGELOG: like
the editor, a workspace without the dependency `.app`s in `.alpackages` reports every
subscriber to a dependency event as `unused_procedure` and `fan_in: 0`.

**routine_id (opus I2, sol 4, sol 5).** `routine_id` equals alsem's `stableRoutineId`
exactly. Two `pub(crate)` helpers are extracted from `l2_workspace::project_file` /
`build_proutine` and called by BOTH them and the CLI, with no other composition anywhere in
the new code:
- `stable_object_id_ir(app_guid, &ObjectDecl) -> Option<String>`: `ir_object_type(kind)?`
  (so the label is alsem's, e.g. `TableExtension`, never `object_kind_label`'s
  `Tableextension`), number `o.id.unwrap_or(0)`, `to_stable_object_id(encode_object_id(..))`.
- `stable_routine_id_ir(stable_object_id, &RoutineDecl) -> String`: `ir_parameter_symbols` →
  `ParamSpec`, return type, `normalized_signature_hash`, `ir_enclosing_member`,
  `to_stable_routine_id_from_parts`.
GUID source: `source_text::read_root_app_guid(project)` (the one l2 uses). `routine_id` is
`null` when there is no app guid (no snapshot either, P5), when `ir_object_type` gives
`None` (PermissionSetExtension, Profile, Entitlement, Other), for Interface/ControlAddIn
members (alsem lists none, P19), and for a nameless routine. If the extraction moves any
golden, that is investigated as a bug; it never licenses a second copy (F2 removed).
CHANGELOG: "equals alsem's `stableRoutineId`; it changes when the app id, the object
type or number, the routine's name, a parameter type or `var`, the return type, or (for a
member trigger) the enclosing member changes. Matching a changed routine across two versions
is left to the consumer" (round 2: no object + name join guarantee; it fails for renames and
cannot separate overloads).

**access / flags.** `access` = `r.access_modifier` (`local`/`internal`/`protected`) else
`"public"` — also for triggers (conductor Q2). `is_trigger` = `r.kind == Trigger`;
`is_event_subscriber` = attribute `eventsubscriber` present (the `ir_walk::ir_routine_kind`
test). `object_id` = `obj.id`; `path` = `rel_posix`.

**Text/CSV unchanged.** They take `graph: None`, so no `unused_procedure` finding exists in
their result at all; the text printer's skip is therefore not needed for correctness, but a
one-line filter of `unused_procedure` stays as a guard (conductor Q1), so a future change
that computes the graph for text cannot leak it.

**Degraded workspace (opus M4).** When `build_full_with` returns `None`, `analyze_project`
writes one line to stderr (`eprintln!`, because JSON mode turns logging off): "warning:
call graph unavailable for <project> (no readable app.json?): fan_in and fan_out are null
and unused procedures are not reported". It makes no claim about `routine_id`, which does
not depend on the graph (round 3). JSON on stdout stays valid.
Null semantics, documented PER FIELD (round 2):
- `fan_in` / `fan_out`: `null` = no graph for this row: the snapshot could not be built
  (then for every row, with the stderr warning) or the row has no matching declaration (a
  join miss). Never `0` for either case. With no graph, the absence of `unused_procedure`
  findings is not evidence that nothing is unused.
- `routine_id`: `null` when there is no app id (no readable `app.json`), AND, even with a
  graph, for routines alsem gives no `stableRoutineId`: Interface and ControlAddIn members,
  objects whose kind `ir_object_type` does not map (PermissionSetExtension, Profile,
  Entitlement, Other), and nameless routines. It does not depend on the graph or the join.
- `object_id`: `null` when the object declares no number.
- `path`, `access`, `is_event_subscriber`, `is_trigger`: never `null`.

### Pre-stated fallbacks

- F1 fan-in definition: kept as stated (the graph; Acceptance 1 says "the resolved call
  graph"). If a reviewer wants `fan_in` = panel rows, the answer is documentation, not a
  second definition.
- F3 join miss: cannot arise from generation skew any more (one parse); a miss gives `null`
  fields and is pinned by the join-null test.
- F4 cost: measured (P18): JSON `--analyze` +2.4 s / +550 MB with `embedded`, +1.2 s /
  +160 MB with `symbols`, on a Base-App-sized dependency set. Text/CSV: +0. Unused verdicts
  were identical under both sources on the probe (P17). Q3 below.

### Acceptance → test

All CLI behaviour tests drive the real binary
(`env!("CARGO_BIN_EXE_al-call-hierarchy") --project <fixture> --analyze --format json
--no-telemetry`), so they pin the USE. New member `tests/cli/cli_analyze_call_hierarchy_json.rs`
(in the `cli` umbrella, which links `al_sem`, so contract checks call the library on the
same fixture in the same test). New fixture `tests/fixtures/analyze-json/` (app.json):
- `src/A/Util.Codeunit.al`: Target calls One TWICE, Two, Three; CallerA calls Target twice,
  CallerB once; `local` One, `internal` Two, `protected` Three, `local` NeverCalled;
  `Over(Integer)`/`Over(Text)`; `[IntegrationEvent]` OnRaised (raised from OnRun),
  `[IntegrationEvent]` OnNeverRaised (neither raised nor subscribed, R4);
  `[InternalEvent]` OnSubscribedOnly (subscribed, never raised, R5); `trigger OnRun` calls
  CallerA, CallerB, `Over(1)`, `Over('x')`, raises OnRaised.
- `src/B/Util.Codeunit.al` (same base name): subscribers to OnRaised and OnSubscribedOnly.
- `src/C/Iface.Interface.al` + an implementing codeunit called through the interface (R6).
- `src/C/Tests.Codeunit.al`: `Subtype = Test`, a `[Test]` procedure and a
  `[ConfirmHandler]` (R3).
- `src/D/Base.Table.al` + `src/D/Ext.TableExtension.al`: a table extension on that table
  with a field `OnValidate` trigger and a procedure that OnRun calls; `src/D/Card.Page.al`:
  a page with two actions, each with `trigger OnAction()` (member-trigger discriminator).
- OnRun also calls the interface method through an interface variable (so the
  implementation is used).
So NeverCalled is the ONLY procedure the LSP rule flags; test 2 asserts that literally, and
any fixture slip shows up there first.
A second fixture `tests/fixtures/analyze-json-noapp/` (no app.json, so the adapter walker
skips it) holds one codeunit.

| # | Test | Discrimination break (Edit, run, revert) |
|---|------|------------------------------------------|
| 1 | `fan_in_and_fan_out_count_distinct`: Target `fan_in == 2`, `fan_out == 3`; plus contract: for every fixture row with a decl, `fan_in` == distinct callers from `handlers::incoming` and `fan_out` == distinct `to` items from `handlers::outgoing` (lib, same fixture; equal here because the fixture has no hidden caller) | count refs instead of callers → fan_in 3; count edges instead of distinct targets → fan_out 4 |
| 2 | `unused_procedure_findings_equal_the_lsp_diagnostics`: binary JSON's `unused_procedure` set of `(path, line)` == `LspSnapshot::build_full(fixture)` + `lsp::diagnostics::compute_all` `unused-procedure` set, AND == `{NeverCalled}` literally (so OnRaised/OnNeverRaised/OnSubscribedOnly, both subscribers, the interface method, Test/handler, triggers are all absent); findings appear after every legacy finding | pass `incoming_count = 0` → many extra; drop the R4 check → OnNeverRaised appears |
| 3 | `path_is_workspace_relative_posix`: the two `Util.Codeunit.al` rows have `path` `src/A/Util.Codeunit.al` / `src/B/Util.Codeunit.al` and still `file == "Util.Codeunit.al"` | emit `file` in `path` |
| 4 | `object_id_access_and_kind_flags`: `local`/`internal`/`protected`/`public` rows, the `OnRun` and `OnAction` rows `is_trigger` with `access "public"`, subscriber rows `is_event_subscriber`, `object_id` per object | default access `""`; swap flags |
| 5 | `routine_id_equals_alsem_stable_routine_id`: run `CARGO_BIN_EXE_alsem fingerprint <fixture> --inventory-only --format json --deterministic`; for every `(objectType, objectNumber, routineName, enclosingMember)` group the multiset of CLI `routine_id`s equals alsem's `stableRoutineId`s (covers Codeunit, TableExtension field trigger, the two Page `OnAction`s, overloads); the two `Over` ids differ; Interface rows are `null`; a second run gives equal ids | use `object_kind_label` for the type → TableExtension mismatch; drop the member → the two OnActions collide; drop params → overloads collide |
| 6 | `legacy_fields_text_and_csv_byte_identical_to_base`: expected files minted ONCE from the BASE binary (`84ba2d4b`, built in a temp worktree) on the same fixture and committed as frozen literals under `tests/fixtures/analyze-json-expected/` (`legacy.json` = base `--format json` output; `text.txt`; `csv.csv`). Mint and test both run with cwd = repo root and the same RELATIVE `--project tests/fixtures/analyze-json`, so the text header `Code Quality Analysis: tests/fixtures/analyze-json` is checkout-independent (round 2, N1). Assert: current JSON with the new keys removed from every row and `unused_procedure` findings removed, compared to `legacy.json` as parsed `serde_json::Value`s (no reliance on `preserve_order`, N2); plus a raw-text check on the current stdout that every row's first nine keys appear in the legacy order and the eight new keys come after `quality_score`; text and CSV stdout == the files byte-for-byte. Not regenerable by `REGEN_TEMP_GOLDENS` (they are the base contract) | drop the text filter AND build the graph for text → text differs; reorder findings → legacy mismatch |
| 7 | `no_app_json_degrades_to_null`: on `analyze-json-noapp`, exit 0, valid JSON, every row has `fan_in/fan_out/routine_id == null`, no `unused_procedure`, stderr has the warning line | return `Some(0)` instead of `None` |
| 8 | `join_miss_gives_null_not_zero` (lib unit test in `analysis.rs`, hand-stated precondition): a snapshot built from a 1-file fixture and a row for a path the snapshot does not hold → `fan_in/fan_out == None`, no unused finding | `unwrap_or(0)` |

Plus: `scripts/check-goldens` byte-identical and `scripts/ci-steps task` green (Acceptance 6);
the 11 `ProcedureMetrics {..}` literals get the new fields; CHANGELOG `Added` entry with the
semantics above (fan_in/fan_out definitions, null meaning, `.alpackages` caveat,
stableRoutineId, dependency-source flag now used by JSON `--analyze`).

### Expected golden moves

**None.** No golden family captures `--analyze` output (P12). `check-goldens` must be
byte-identical; the stable-id extraction is a pure move guarded by the L2/R1a goldens. The new
`analyze-json` fixture is also swept by `adapter_loses_nothing_cross_app_on_the_fixtures`
(not a golden; must stay green). The frozen base outputs of test 6 are new files, not moves.

### Questions for the conductor

- Q1 answered: text skips `unused_procedure`. Q2 answered: triggers `access: "public"`.
- Q3: JSON `--analyze` default dependency source. Design: honour `--dependency-source`,
  default `embedded` (the LSP's graph; +2.4 s / +550 MB per run with Base App). Alternative:
  default `symbols` for `--analyze` only (+1.2 s / +160 MB; identical unused verdicts in P17,
  but the panel would hide symbols-only callers more often). CentralGauge can pass
  `--dependency-source symbols` itself either way.
- Q4: do CentralGauge's agent workspaces carry `.alpackages`? Without them every subscriber
  to a Base App event is reported `unused_procedure` (P17). No code change either way.

### Discoveries (not in scope)

- D1: `engine::snapshot::snapshot_workspace` composes `stableRoutineId` a second time
  (`params_ir`, P20), and `program::model::workspace` a third (`to_stable_routine_id_from_parts`
  at `src/program/model/workspace.rs:1352` — the routine id `alsem fingerprint --inventory-only`
  emits; `:932` builds the Interface/ControlAddIn ids the inventory does not list). Both could use the shared helpers; left alone here (R0 differential goldens, model
  build). Test 5 pins `--analyze`'s ids to the inventory, so a drift between them fails.

## Implementation

One task (bounded). Files: `src/analysis.rs` (facade `analyze_project`, `file_metrics`,
`Graph`, `object_kind_label` moved from main.rs, new fields, test 8), `src/main.rs`
(`run_analysis` plumbing, help text, text-printer filter; `extract_metrics_ir` and
`object_kind_label` removed), `src/lsp/handlers.rs` (`incoming_callers`, which `incoming` now
calls; `outgoing_targets_by_caller`), `src/lsp/diagnostics.rs` (`is_unused_procedure`
`pub(crate)`), `src/program/body/l2_workspace.rs` (`stable_object_id_ir`,
`stable_routine_id_ir`, used by `build_proutine`, `project_file`, `project_named_routine` and
the CLI), `CHANGELOG.md`.

Test 8 could not be run red on base: it calls `file_metrics`/`Graph`, which base lacks (a
compile failure, not an assertion). Its red run is break B14 below. All 8 green after the
implementation (`proof.sh`).

### Discrimination proofs

Each break was made with Edit, the 8 tests run (`.agent/runs/20261023-090537-f1612b/proof.sh`),
then reverted with Edit. Breaks marked "+" were applied together because they hit different
tests. After the last revert all 8 pass and `git diff` holds no break residue (grep).

| # | Break (Edit) | Failing test(s), assertion | After revert |
|---|--------------|----------------------------|--------------|
| B1 | `fan_in` = `snap.incoming_count(id)` (references, not callers) | test 1 `Target fan_in`: 3 vs 2 | 8/8 pass |
| B2 | `fan_out` = number of call edges from the routine in its file (not distinct targets) | test 1 `Target fan_out`: 4 vs 3 | 8/8 pass |
| B3 | `outgoing_targets_by_caller` drops `ws_event_edges` | test 1 `fan_out vs panel` for line 47 (OnRaised): 0 vs 1 | 8/8 pass |
| B4 | `is_unused_procedure(d, r, 0)` | test 2 `unused_procedure findings`: + Target, CallerA, CallerB, One, Two, Three, … | 8/8 pass |
| B5 | shared rule R4 keeps only `Business` (drops `Integration`) — both CLI and LSP move | test 2 literal set: + `Probe Util.OnNeverRaised` | 8/8 pass |
| B6 + B7 | unused findings placed BEFORE legacy ones + `path` = base name | test 2 `must come after all legacy findings`; test 3 `left: "Util.Codeunit.al"`; test 1 `no row with path src/D/Card.Page.al line 22` | 8/8 pass |
| B8 + B9 | CLI passes a zero GUID to `stable_object_id_ir` + `access` default `""` | test 5 `routine_id vs alsem stableRoutineId` (`00000000-…:Codeunit:50100#…`); test 4 `access of …`: `""` | 8/8 pass |
| B10 | shared `stable_routine_id_ir` drops the enclosing member | test 5 `routine_id vs alsem stableRoutineId` | 8/8 pass |
| B11 + B12 | graph built for every format + text filter removed; no-graph `eprintln!` disabled | test 6 `text output changed` (extra `[INFO] … is never called` line); test 7 `stderr must say why the graph fields are null` | 8/8 pass |
| B13 + B14 | `path` moved before `quality_score` in the struct + `fan_out: Some(…unwrap_or(0))` | test 6 `row key order` (`… parameter_count, path, quality_score …`); test 8 `join miss: fan_out must be None, not 0`; test 7 `fan_out must be null without a graph` | 8/8 pass |

### Gates and commit

- First `ci-steps task-affected`/`ci-steps test` run: 2 failures, both the fixture-count
  tripwire `FIXTURE_ROOTS` (`("fixtures", 33)` → 34 app dirs, the new `analyze-json`
  fixture; the tripwire's own doc says to bump it). Bumped to 34 in
  `src/engine/l4/capability_cone.rs`; `derived_only_matches_raw_fold_on_fixtures` and
  `relation_equals_the_legacy_base_per_field` then pass WITH the new fixture in their sweep.
- Final: `scripts/ci-steps task-affected` exit 0 (3800 run, 3800 passed, 8 skipped);
  `scripts/ci-steps test` exit 0 (3800/3800, 8 skipped); `scripts/check-goldens` exit 0, no
  golden file modified (`git status`); `scripts/ci-steps clippy` (release, -D warnings) exit 0,
  rerun after the last change. `CDO_WS` unset.
- Commit `cba0a7eb` on base `84ba2d4b` (origin/master unchanged at fetch). `agentflow
  check-diff --base origin/master --head HEAD --issue 159` → `{"reasons": []}`.
- Cost (release-fast, 3 runs each): fixture `--format json` 0.01 s / 17-18 MB, `--format text`
  0.00 s / 15-16 MB. `cg-ws` (3 files + Microsoft System/Base/Application `.app`s): json
  `embedded` 2.51-2.62 s / 578-582 MB; json `symbols` 1.18-1.23 s / 174 MB; text 0.00 s /
  12-14 MB (no graph). On `cg-ws` the JSON matches the LSP probe P17: the Base App subscriber
  `fan_in: 1`, the misspelled one `unused_procedure`.

Not discriminated (stated limit): building rows from `snap.parsed` instead of re-reading the
file. On a quiet tree both give the same text, so no test can tell them apart; the join is
correct by construction (one parse).

## Final panel round 1 fix-forward

Commits: `fd1c9029` (the #159 commit rebased onto `6b09105b`), `c5fc7d83` (this fix-forward).
Gates on `c5fc7d83`'s tree: `ci-steps task-affected` 0 (982/982), `check-goldens` 0 (no golden
moved), `ci-steps test` 0 (3810/3810, 8 skipped), `ci-steps clippy` (release) 0. `check-diff
--base origin/master --head HEAD --issue 159` → `{"reasons": []}`. Not pushed. Register:
FQ-01..FQ-12 fixed, CR-5 refuted, CR-8/CR-10 deferred (anchor_hash `c5fc7d83`).

Rebased onto origin/master `6b09105b` (#78; clean, both CHANGELOG entries kept) → `fd1c9029`.
Fixes per `final-r1-fix-brief.md`, each checked on the code first:
- FQ-03: Interface/ControlAddIn rows: `fan_in`/`fan_out` null (`counted = decl.filter(!signature_only)`);
  the unused rule still sees the decl (R6 is the LSP's own exclusion; ControlAddIn members stay
  under the LSP's rule).
- FQ-04: `in_nested_app` (a folder below the project with its own `app.json`) → no app guid →
  `routine_id` null for that file. New test 9 `nested_app_files_get_no_routine_id` (tempdir
  workspace; the root file keeps its id). No new fixture dir (a nested fixture would add 2 app dirs
  to the `FIXTURE_ROOTS` and adapter sweeps).
- FQ-05: warning reason: `no app.json at the project root` or `the program snapshot could not be
  built` (the builder returns no reason; the missing root app.json is the one case we can tell).
- FQ-06: `Finding.path: Option<String>` (`skip_serializing_if = None`), set on `unused_procedure`
  only; legacy findings have no `path` key (test 2 asserts it; test 6 compares them to base).
- FQ-11: per-file `HashMap<start byte, &DeclEntry>` instead of a linear find per routine. The
  per-routine caller grouping allocation stays (shared helper with the panel; O(refs)).
- FQ-12: text-printer filter removed; a `debug_assert!` states the invariant (text never builds
  the graph).
- Fixture (FQ-08): `Dispatch(AnyGreeter: Interface "Probe Greeter")` appended to Util (called
  from OnRun) and `src/C/GreeterImpl2.Codeunit.al`; NeverCalled stays the only unused
  procedure; lines of existing routines unchanged. Frozen base outputs RE-MINTED with the base
  binary built from `git archive 84ba2d4b` (`/tmp/i159-base-target`), cwd = worktree, relative
  `--project tests/fixtures/analyze-json`, empty `HOME`; two runs `cmp`-identical.
- Tests: FQ-01 test 5 pins the two `Over` rows (lines 38, 42) and the two `OnAction` rows
  (Card.Page.al 22, 28) to ids computed by hand from `engine::ids` (`normalized_signature_hash`
  + `to_stable_routine_id_from_parts`); FQ-02 test 7 asserts the exact warning line and no
  `routine_id` in stderr; FQ-03/08 test 1 asserts interface `Greet` null and `Dispatch.fan_out
  == 2`; FQ-06/09 test 2 compares (workspace-relative path, line) with `compute_all` (uri minus
  the snapshot root uri); FQ-07 every binary run (both binaries) gets an empty `HOME`,
  `USERPROFILE`, `XDG_CONFIG_HOME`.
- CHANGELOG (FQ-10): nested-app and signature-only nulls, alsem digest note, finding `path`,
  warning reasons, cost +2.5 s / ~565 MB, no "one shared implementation" claim for the stable id.

### Discrimination proofs (final r1)

All 9 tests per run (`proof.sh`); reverted with Edit after each; 9/9 pass after the last revert;
`git diff` grep for every break shows no residue.

| # | Break (Edit) | Failing test(s), assertion | After revert |
|---|--------------|----------------------------|--------------|
| F1 + F2 + F4 | CLI mints each routine's id from the OTHER same-named routine (swaps the two `Over` and the two `OnAction` ids; the multisets stay equal) + old warning text claiming routine_id + nested filter disabled | test 5 `routine_id of src/A/Util.Codeunit.al:38` (the multiset check passed first); test 7 `stderr must carry exactly …`; test 9 `a nested-app file must not get an id minted with the root's guid` | 9/9 pass |
| F3 + F6 + F12 | signature-only rows counted + unused finding `path: None` + graph built for text | test 1 interface `Greet` `fan_in` 0 vs null; test 2 `unused finding carries path`; test 6 text run panics `text output must not get unused_procedure findings` (the debug_assert) | 9/9 pass |
| F8 | `outgoing_targets_by_caller` keeps only the first target of each edge | test 1 `every candidate counts`: Dispatch `fan_out` 1 vs 2 | 9/9 pass |
| F11 | decl map keyed by END byte (every join misses) | test 1 `Target fan_in` null; test 2 `unused_procedure findings: []`; test 8 precondition `(1, None, None)` | 9/9 pass |

Not discriminated: FQ-05's second reason (`the program snapshot could not be built`) has no
test; FQ-07's empty HOME is test hygiene (proving it would need a config in the real home).

## Base/Head SHAs

- B (base, `origin/master`): `6b09105b` (#78's merge; rebased from `84ba2d4b`; original #159 commit now `fd1c9029`).
- H (gated head): `c5fc7d83` (final-panel round-1 fix-forward).
- After H: the evidence commit (ledger + register) only.

## Final panel

Rounds: 2 (final_rounds). Round 1: sol (2 minor), Opus (6 minor), code-review at high (10 candidates); consolidated
FQ-01..FQ-12 fixed in `c5fc7d83`; CR-5 refuted by design; CR-8, CR-10 deferred (minor). Round 2: both accepted all 15.
Opus's FQ-10 ledger nit (D1 cited `workspace.rs:932` for the inventory id) corrected here (`:1352`), ledger only.

## Findings register summary

31 entries: spec panel R1-01..R1-14, R2-01, R2-02 (16, 3 rounds) and final panel FQ-01..FQ-12, CR-5, CR-8, CR-10
(15, 2 rounds). 0 open; refuted: CR-5; deferred (minor): CR-8, CR-10. All accepted by both reviewers.

## Acceptance matrix

| # | Result | Evidence |
|---|---|---|
| 1 fan_in / fan_out | delivered | tests 1 (Target 2/3, Dispatch fan_out 2, vs the panel) |
| 2 unused_procedure | delivered | test 2 (only NeverCalled; equals `compute_all` on the same fixture; `path`) |
| 3 path | delivered | test 3 (two same-name files, distinct `path`, `file` unchanged) |
| 4 object_id / access / flags | delivered | test 4 |
| 5 routine_id | delivered | test 5 (per-row equality with alsem's inventory `stableRoutineId`, overloads + member triggers); test 9 nested app null |
| 6 text unchanged / regression | delivered | test 6 (legacy JSON as values + key order, text and CSV byte-identical vs base), check-goldens unchanged, ci-steps task green |

## Gate results

On H `c5fc7d83` (recorded by `agentflow run`, clean before/after): `ci-steps-all` 0, `cdo-gate` 0,
`check-goldens-coverage` 0, `check-goldens` 0.

## Discoveries

```json
[
  {"subsystem": "program identity (stableRoutineId builders)", "locator": "src/program/model/workspace.rs:1352 to_stable_routine_id_from_parts; src/engine/snapshot.rs params_ir; src/program/body/l2_workspace.rs stable_routine_id_ir", "symptom": "stableRoutineId is composed in three places (the new shared l2 helper, program::model::workspace, engine::snapshot); they agree today (test 5 pins --analyze vs the inventory) but can drift (#159 D1).", "kind": "enhancement", "origin_issue": 159, "pre_existing": true, "reproducer": "grep stable routine id composition in the three files.", "capability": "one shared stableRoutineId helper used by all three", "acceptance": "workspace.rs and engine/snapshot.rs call the shared helper; goldens unchanged"}
]
```

## Not delivered

Nothing in the Acceptance. Stated limits (CHANGELOG): fan_in counts graph callers (the panel hides unlocatable ones);
Interface/ControlAddIn members and nested-app files get null graph/identity fields; without dependency symbols in
`.alpackages`, Base App event subscribers are reported unused (symlink tip); JSON builds the call graph (+2.5 s /
~565 MB embedded).
