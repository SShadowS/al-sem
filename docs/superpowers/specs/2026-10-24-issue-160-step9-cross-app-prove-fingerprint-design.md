# #160 step 9: prove and the fingerprint query cross-app (scope S′)

Status: spec, attempt 10, 2026-10-24. Base `b028ec32` (master with step 6 merged, #274).

Child of `2026-10-18-issue-160-analysis-session-design.md` (the epic): §9 row **9**; E6, E7, E9, E12;
§5.10, §5.12; §11 "false PROVEN", "a cone reaching a routine with no model row must fail closed (step 9
traces it)"; open questions 6 and 10. Child of step 6's spec
`2026-10-23-issue-160-step6-one-cone-graph-design.md` (6A-9, §14.2, §14.3.5, §14.13 decision 4), which hands
this step the prove and fingerprint arms of the scoped-equals-unscoped oracle and the switch of prove and
fingerprint off the cut.

Every citation is at `b028ec32`, as `file:line` plus the line's text, so it can be found after the code moves.
Every number is cited from a file (path given) or read in this attempt's probes (§9). An estimate is labelled
**estimate** and shows its arithmetic.

**Step 9 is a SPEED step** (epic row 9: "both peaks ≤ parent cross-app analyze + band"). Its bounds are peak
bounds. Time and output size are reported, not bounded.

## Words

- **Universe**: `single-app` (the workspace alone) or `cross-app` (the workspace plus the dependency code it
  demands). **Default**: what a command does with no `--single-app`.
- **Role**: `Primary` or `Dependency` (`Model::routine_role`, #220).
- **View**: the session object that holds the model and its cells (`src/engine/session.rs`).
- **Witness request**: the routines whose raw inherited facts the view's one composition keeps
  (`session.rs:89` `pub enum WitnessRequest {`). Its subjects are the **subjects** of the composition.
- **Tree**: a `CapabilitySnapshot` built by `compose_tree` (`snapshot.rs:1374` `pub(crate) fn compose_tree(`)
  over the view's cone graph (`rel.cone()`).
- **Cut**: the old single-app snapshot, `compose_snapshot` over `build_r3a3_source_only_base` (workspace
  callers only). The cut is what prove and fingerprint read today.
- **B0**: fingerprint's full-snapshot path (json with no query flag, cbor, cbor.gz, `--shard`).
- **Query path**: fingerprint's json-with-query-flags and human forms (`fingerprint_query`).
- **PROVEN**: a prove answer that is not `unknown`. **False PROVEN**: a PROVEN answer the program does not
  support (E6).
- **Rule B**: step 6 §14.3.5. A witness stops at the first dependency routine. Effects reached only through it
  are grouped per (boundary, effect type) with `conditionality: unknown`. Every tree carries it
  (`boundary: Some(boundary)` in `compose_tree`); the cut never does.
- **P_a**: the parent's cross-app `analyze` kernel peak (`VmHWM`, median of 3), the bound's reference.

## Operator decisions (decided before the claim)

Source: `/work/bench/160-9/OPERATOR-DECISIONS-2026-10-24.md`; probe record
`/work/bench/160-9/probe-out/OD1-RESULT.md`.

- **OD-1 = S′.** The pre-stated rule chose F (fold dependency-origin facts) only if the fold left ≤ 10 % of the
  raw facts AND B0 projected within P_a + 1.5 GiB AND `diff` within P_a + 2.5 GiB. Measured: the fold keeps
  **97.65 %** (8,283,963 of 8,483,157 facts; ≥ 97.48 % under any carrier attribution), because the fold key
  holds the subject and the facts are already nearly distinct per subject. Fingerprint B0 cross-app was
  **killed by the shared 32 GiB cgroup at ≥ 29,113,888 KiB** before it serialized. So: **step 9 ships prove,
  the fingerprint query with `--routine`, and `fingerprint --inventory-only` cross-app.** Fingerprint B0 and
  `diff` cross-app are a filed follow-up (ledger `## Discoveries`), which must not rely on the fold as the
  pre-draft's D9 wrote it.
- **OD-2 = yes.** If prove misses `≤ 1.03 × P_a`, it is read against digest's accepted bound B2′ (digest
  parent peak + 3.0 GiB) (F6).
- **OD-3 = accept.** Rule B applies in prove and the fingerprint query. One witness rule everywhere. A
  must-question that can only be answered through dependency code answers `unknown`.
- **OD-4.** A bench round is VOID only when the EXTERNAL load (1-min load minus the measured process's own
  running threads) exceeds 8.
- **Memory ceiling.** Every bound and every probe lives inside the shared 32 GiB cgroup, not the host's RAM.

## 1. Goal

1. **Three forms become cross-app by default**: `alsem prove`, `alsem fingerprint` with at least one
   `--routine`, and `alsem fingerprint --inventory-only`. Each builds the cross-app `View` and reads a tree
   composed over `rel.cone()`, as digest does since step 6. Behaviour follows calls into dependency code;
   inventory rows stay primary (#220's key classes).
2. **`--single-app`** on `prove` and `fingerprint` restores the base's output byte for byte on these forms.
3. **The query path filters by role** (epic Decision 28): a selector never resolves to a dependency routine,
   and no root block is rendered for a dependency routine.
4. **No false PROVEN cross-app** (E6): a cone that enters body-less, ABI-only or opaque dependency code blocks
   the answer.
5. **Prove and the query path stop reading the cut.** After this step `compose_snapshot` has no reader in
   `prove.rs`, and the `--routine` query path does not call it.

## 2. Non-goals

- **Fingerprint B0 and `diff` stay single-app** (OD-1 = S′). Their code path is unchanged. So are the
  query forms with no `--routine` (human with no flag, `--roots` alone, `--witness` or
  `--no-include-inherited` alone): they render every root classification (3,908 on CDO, §8.0), which costs
  about as much as B0's composition. They move with B0 in the follow-up.
- `diff` gains no `--single-app` flag in this step: the flag would be inert. The follow-up adds it.
- No change to resolution, demand, the cone, coverage, roles, key classes, the gap rule, the DAG witness rule
  or rule B (steps 5, 6, 8 own them). Histogram SHA and analyze SARIF are unchanged.
- The ordering cell stays on the cut (step 6 §14.2, #161).
- No change to `unresolved_cone`'s caps (`unresolved_cone.rs:55` `pub(crate) const DEFAULT_MAX_DEPTH: u32 = 64;`,
  `:56` `const DEFAULT_MAX_VISITED: usize = 25_000;`). P4 shows them unreached on CDO.
- No universe statement in prove's or the query's output (neither carries one today; digest does not either).
- No prove-local workaround for a ledger, gap or rule-B defect (F8).
- No `WitnessRequest` change (§4.7).
- No batch (step 12).

## 3. Acceptance matrix

| # | Epic row 9 / contract item | Evidence |
|---|---|---|
| A9.1 | the cross-app `View` for the commands | prove: §4.2, T1, T2, T8. Query `--routine`: §4.3, T3, T8. Inventory-only: §4.4, T8, T11. Fingerprint B0 and `diff`: **not deliverable in this step, because** OD-1 measured B0 at ≥ 29.1 GB (killed); follow-up filed |
| A9.2 | `--single-app` | T8; §8.3 Part 1: parent default == head `--single-app`, byte for byte, for `pv`, `fq`, `fi` |
| A9.3 | three stated output moves | (1) diagnostics source: T14; P1 shows 0 lines on CDO in both universes, so CDO cannot show the move. (2) dependency shards: **not deliverable**, shards are a B0 form (follow-up). (3) `unresolved_cone` caps: T15 plus P4's visited distribution (max 19,284 of 25,000; 0 truncated) |
| A9.4 | fixtures (a), (a'), (b), (c), (d) | (a) T1; (a') T2; (b) T3 on the query path; (d) T5, T6. (c) dependency-mediated `capability-gained` is a `diff` fixture: **not deliverable**, follow-up |
| A9.5 | one discrimination per filter | the role filters: T5, T6. The universe switches: T1-B1, T3-B, T8, T21. The gap count: T1-B2, T2-B2. Rule B in prove: T19 |
| A9.6 | no-dependency parity (E9) | T7 over every no-dependency r0 dir, for prove, the `--routine` query and inventory-only |
| A9.7 | CDO fingerprint time and size reported | §8.3 Part 3: `fq`, `fi` and single-app B0 time and size. Cross-app B0: the OD-1 lower bound (≥ 29.1 GB, killed, no output) is the report |
| A9.8 | both peaks ≤ P_a + band | §8.1: B2 (prove), B1q (`--routine` query), B1i (inventory-only) as worded. B0 and `diff`: no-regression bounds B1s, B3s (they stay single-app) |
| A9.9 | cell census | §8.3 Part 1: `[retained-census] view.live cells=…` and `witness.non_primary_members=0` per cross-app run |
| A9.10 | a miss walks §5.12's ladder | §7 |
| A9.11 | goldens: cli-b header `cross-app`; contents unchanged | cli-b prove, fingerprint and diff goldens: **byte-identical** (no cli-b fixture has a dependency, and the forms with a header, B0, stay single-app). The `cross-app` header in cli-b B0 files moves to the follow-up |
| E6 | never falsely PROVEN | T1, T2, T12, T15, T19; §6 |
| E7 | behavioural fingerprint/diff cross-app | the query half: T3. The B0 and `diff` half: follow-up |
| E9 | parity | T7 |
| E12 | no dependency subject materialized | every cross-app run in Part 1 prints `witness.non_primary_members=0` |
| 6A-9 | prove and fingerprint arms of scoped-equals-unscoped | T17 |
| E14 / I10 | CDO ratchets | `scripts/cdo-gate` |

### Expected output moves, and how each is triaged

| Where | Move | Triage |
|---|---|---|
| cli-b goldens (prove, fingerprint, digest, diff, snapshot) | none | byte-compare; any move fails |
| `tests/cli/cli_build_counters.rs` rows | prove and the `--routine` query lose the standalone composition; inventory-only composes no raw facts (§4.6) | each row read at the base |
| `tests/cli/cli_p1_inventory.rs` | `payload.analysisUniverse` is `"cross-app"` by default (the test pins `"single-app"` today, `:110`) | the test is rewritten with the code it pins; its `--single-app` twin keeps `"single-app"` |
| CDO prove, cross-app | answers can move from `no`/`yes` to `unknown`. P4: `d`'s two large roots carry 420 and 419 unresolved items cross-app, against 154 summed over all three roots single-app. Dependency effects appear as rule-B groups | a move TO PROVEN (from single-app `unknown`) without a new primary effect is a defect (F8) |
| CDO `fingerprint --routine R` | inherited dependency facts appear on R; witnesses stop at the boundary | reported; a primary fact lost is a defect |
| CDO inventory-only | `coverage` statuses can change where a primary routine's cone enters dependency code; `apps[]` gains the dependency apps owning a kept identity | reported per key; a primary routine row lost is a defect |

## 4. Design

### 4.1 D1: one target builder, `--no-roots-config` cross-app, and the flag

- `AnalysisTarget::build(inputs, single_app, command)` (`run.rs:180` `pub fn build(`) has no roots-config
  parameter: its single-app arm passes a literal `false` (`run.rs:190-194`), and `build_cross_app_base`
  (`capability_cone.rs:3807` `pub(crate) fn build_cross_app_base(`) passes a literal `false` to
  `assemble_and_resolve_cross_app_program(inputs, model_instance_id, false,)` (`:3812-3816`). Fingerprint
  honours `--no-roots-config` today only because it builds single-app
  (`assemble_and_resolve_workspace_program(opts.inputs, model_id, opts.no_roots_config)`, `fingerprint_cli.rs:375`).
- Add `AnalysisTarget::build_with(inputs, single_app, no_roots_config, command)` and
  `build_cross_app_base_with(inputs, model_instance_id, no_roots_config)`. The existing functions become
  one-line wrappers passing `false`. The 11 `AnalysisTarget::build(` and 19 `build_cross_app_base(` call
  sites are untouched.
- `ProveCli` (`alsem.rs:533` `struct ProveCli {`) and `FingerprintCli` (`alsem.rs:559` `struct FingerprintCli {`)
  gain `--single-app`, with digest's help text (`alsem.rs:661` `/// Analyse the workspace alone. The default
  loads the dependencies it requires`) plus, for fingerprint, one sentence: "Applies to `--routine` and
  `--inventory-only`; the other forms analyse the workspace alone in this release."
- `FingerprintOptions` gains `single_app: bool`; `run_prove_pipeline` (`prove.rs:840`) gains `single_app: bool`.
  Every construction is updated (17 `FingerprintOptions {` sites, 6 `run_prove_pipeline(` sites; recount at the
  task). A site that means today's behaviour passes `true`.
- Failure texts are `AnalysisTarget::build`'s: `"{command}: could not compute modelInstanceId"`,
  `"{command}: workspace did not resolve"`, and `cross_app_failure_message` (`run.rs:206`), whose head already
  names `--single-app`. Exit codes are unchanged.

### 4.2 D2: prove is digest with one root

`prove_pipeline_with` (`prove.rs:859`) today builds a single-app view
(`let mut view = View::new(AnalysisTarget::SingleApp(Box::new(resolved)));`, `:880`), runs a no-root
`digest_plan` (`:881`), then composes the cut (`let mut snap = compose_snapshot(resolved);`, `:888`). It
becomes digest's pipeline (`digest_cli.rs:1724-1800`) with one selector, in both universes:

1. `AnalysisTarget::build(inputs, single_app, "prove")`, `View::new(target)`.
2. The root is resolved before `prepare`, in a block that ends before it: `derive_identity_table(model)`,
   `OwnerIndex::build(model)`, `resolve_changed_roots(&identities, model, &changed_input,
   primary_guid_of(model), &owners)` (digest's block, `digest_cli.rs:1761-1771`).
3. `digest_plan(detectors, roots, false)`; `prepare`; `finish_default_run`; `view.take_effects()` when
   `plan.needs_tree()`.
4. **Zero roots**: the plan has no tree (digest's no-root case composes none,
   `cli_build_counters.rs` `fn digest_with_no_resolved_root_builds_no_tree`). The dummy document and exit 2 are
   written as today, with the run's `diagnostics_json`.
5. **The gap count moves into the effects.** Prove counts gaps today from the cut
   (`let cone_result = unresolved_cone(&snap, root_id);` and `snap.analysis_gaps`, `prove.rs:955-962`). Under
   digest's plan the tree is dropped inside `prepare` (`*tree = None;` then
   `note_release(released, Cell::Tree);`, `session.rs:546-547`), and `UnresolvedCone`
   (`digest_cli.rs:785` `pub struct UnresolvedCone {`) keeps only `items` and `traversal`. So `UnresolvedCone`
   gains `gaps_in_cone: usize`, computed in `digest_effects` (`digest_cli.rs:794`) from the same
   `unresolved_cone(tree, r)` call's `visited_ids` and `tree.analysis_gaps`, before the tree is dropped.
   `count_gaps_in_cone` (`prove.rs:168`) moves to `digest_cli.rs` unchanged; its app-level prefix match is the
   soundness rule its doc states. `UnresolvedCone` has no `Serialize`; digest never prints the field, so digest
   output cannot move.
6. Everything after (`DbTempStates` off `view.l4()`, `run_digest_query_full_from_entries`, `prove(…)`, the
   projection) is unchanged.

Single-app, the tree is over `rel.cone()`, which equals the cut's edges (step 6 P13), and digest single-app
already runs this path. P15 (§9) found every cli-b prove golden's evidence equal to digest single-app's
effects for the same routine. Cross-app, rule B applies (OD-3): a dependency effect is a boundary group with
`conditionality: unknown`, and a boundary occurrence never satisfies a MUST condition (step 6 §14.3.5 item 4).

### 4.3 D3: the fingerprint query with `--routine`

**Which forms.** `run_fingerprint_pipeline` (`fingerprint_cli.rs:358`) routes to the query path when no B0
condition holds (`let go_b0 = opts.shard.is_some() || …`, `:416-419`). In that path, the query is **in scope**
when `!opts.routine_selectors.is_empty()`. In-scope forms take the new path in both universes. Every other
query form keeps today's path exactly (single-app, the cut) whatever `--single-app` says (§2).

**The new path:**
1. `AnalysisTarget::build_with(opts.inputs, opts.single_app, opts.no_roots_config, "fingerprint")`,
   `View::new(target)`.
2. The selectors are resolved before `prepare`, from the model alone: `derive_identity_table(model)`, the
   selector index built with the primary app guid (Decision 28, below), `resolve_selector` and
   `narrow_to_owner` as `fingerprint_query` does (`fingerprint_query.rs:663-664`). The set of ids that
   resolve to exactly one routine is the pool. A failed selector adds nothing; its diagnostic comes from step 4.
3. The plan is `fingerprint_query_plan(detectors, pool)`: `fingerprint_plan`'s run plus
   `WitnessRequest::Roots(pool)` and a kept tree (`TreeUse::Keep`, §4.5). `prepare`; the run's analyzer
   diagnostics (`analyzer_diags_for(&view, &key)`, as today, `fingerprint_cli.rs:391`); `view.take_tree()`;
   then `release_all_but(&[])`.
4. `fingerprint_query(&tree, &filters, roots_config_ignored, &OwnerIndex::build(model))` reads the kept tree.
   It resolves the selectors again over `tree.identities`, which is the same `derive_identity_table` output,
   so the diagnostics (unresolved, ambiguous, the #239 owner hint) are unchanged. The rest of the query path
   (from `let snap = compose_snapshot(resolved);`, `fingerprint_cli.rs:499`, on, minus that line) is unchanged.
5. The `--strict` gate stays before any output, as today (`fingerprint_cli.rs:398-413`).

**Decision 28 (role).** Two filters, each with its own test:
- the selector index takes `primary_guid_of(model)` instead of `None` in both places that build it
  (`build_selector_indexes(&snap.identities, None)`, `fingerprint_query.rs:655`, and the human error path,
  `fingerprint_cli.rs:538`), as digest and prove pass it (T5);
- the root pool keeps a `root_classifications` row only when its routine is `Role::Primary`, read from the
  model row (`Model::routine_role`), not from a guid prefix; `total_classifications` counts the filtered pool
  (T6). `derive_root_classifications` (`snapshot.rs:2866`) has no role filter, so a cross-app tree can hold
  dependency root rows. Single-app every row is primary, so nothing moves.

`fingerprint_query` takes the model's role through a new parameter (`primary: Option<&str>` plus a
`is_primary(stable_id)` closure built from the model), not by reading the model itself, so its existing
callers (the cut path, tests) pass the single-app values unchanged.

### 4.4 D4: `--inventory-only` on a tree with no raw facts

Today inventory-only goes down B0: `compose_full_snapshot(resolved, &full_opts)` (`fingerprint_cli.rs:438`),
then `build_inventory_envelope(&tree, …)` (`:467`). So it composes the whole cut snapshot and every primary
routine's raw facts, then reads four keys (`apps`, `identities`, `coverage`, `rootClassifications`,
`snapshot_full.rs:1336` `pub fn build_inventory_envelope(`).

1. **Split `compose_full_snapshot`** (`snapshot_full.rs:137`) into the cut half and an assembly half:
   - `compose_full_snapshot(resolved, opts)` keeps its signature and callers (B0, `diff`, 9 call sites):
     it builds `let base = build_r3a3_source_only_base(resolved);` (`:163`), `core`, and
     `derive_permissions(resolved, &base)` (`:174`), then calls the assembly half.
   - `assemble_full_snapshot(resolved, core, permission_facts, opts)` is everything after: contracts,
     schema, `reachable_gaps`, `filter_by_class`, `project_apps`, and the CBOR map in its literal key order.
     `reachable_gaps` (`:507` `fn reachable_gaps(`) and `filter_by_class` read only `routine_to_stable` from the
     cut base; both take `routine_to_stable_of(ws)` (`capability_cone.rs:4555`) instead, the one builder the
     cut base's own field is made with (`:4647` `let routine_to_stable = routine_to_stable_of(ws);`). So B0 and
     `diff` bytes cannot move (T18 pins it).
2. **Inventory-only** builds the target with `build_with(.., opts.single_app, opts.no_roots_config, ..)`, the
   plan `fingerprint_inventory_plan(detectors)` = `fingerprint_plan`'s run plus `WitnessRequest::Roots(∅)` and
   `TreeUse::Keep`, takes the tree, and calls `assemble_full_snapshot(model, tree, CborValue::Array(vec![]),
   opts)` with `universe: SnapshotUniverse::of(view.target())`, then `build_inventory_envelope` unchanged.
   `Roots(∅)` composes no raw facts (digest `--tests`' shape, `session.rs:166-170` doc), so
   `build.witness_pass` stays 0 (`capability_cone.rs:3221-3222`: the counter moves only when `raw` is not
   empty).
3. **What the inventory says cross-app** (a stated definition, 9A-3): the inventory projection of a tree
   whose raw scope is empty. `permissionFacts` is empty because it needs every primary routine's raw facts
   (`derive_permissions` reads `base.raw`, `snapshot_full.rs:950-951`), and the inventory does not print it. It
   is passed to `filter_by_class` only as a source of references, so a dependency identity or app referenced
   ONLY by a permission fact or an inherited fact is not kept. Every primary row is kept (`filter_by_class`
   keeps every primary id, step (5)).
4. **Single-app** takes the same tree path. T7 (no-dependency parity) and T8 (`--single-app` == the base's
   bytes) compare it with today's cut path on every r0 dir; F9 is the fallback if they differ.

### 4.5 D5: a kept tree in the `View`

Today a tree exists only for digest: `assert!(!plan.needs_tree || matches!(plan.witness,
WitnessRequest::Roots(_)), "a tree is composed over a Roots witness request");` (`session.rs:407-410`), and it
dies inside `prepare` after `digest_effects` (`session.rs:540-547`). `Cell::ALL` leaves `Tree` out because it
never outlives `prepare` (`session.rs:139-142`, `const ALL: [Cell; 5] = [Cell::Det, Cell::L4, Cell::Ordering, Cell::Rel, Cell::Raw];`).

- `Plan.needs_tree: bool` (`session.rs:81`) becomes `tree: TreeUse`, `enum TreeUse { None, Effects, Keep }`.
  `Effects` is today's digest behaviour. `Plan::needs_tree()` (`:281`) stays, as `tree == TreeUse::Effects`,
  for digest's one reader.
- Under `Keep`, `prepare` composes the tree in the same block and order as today (after the composition,
  before `det` takes `caps`), skips `digest_effects`, and leaves the tree in `Cell::Tree`. The dependency
  calls are taken in the same first prepare (`let dependency_calls = match &mut *target {`, `session.rs:468-473`; final panel F1-C2 of step 6: a second
  prepare cannot get them back).
- `View::take_tree(&mut self) -> CapabilitySnapshot` moves it out and marks `Cell::Tree` released; it panics
  with a message if no tree was kept. `Cell::ALL` gains `Tree`, so `release_all_but` cannot leak one.
- The assertion keeps its rule: a tree (either use) is composed over a `Roots` request.
- **Retention (epic Decision 15).** A kept tree is alive through `det` and the run. For one `--routine` root
  it holds that root's raw facts plus the tree's skeleton (typed edges, ledger, indexes over the cone); for
  inventory-only it holds the skeleton alone. §8.0 prices it; F2 is the pre-stated fallback.

### 4.6 D6: subjects and counters per command (I1, I5)

| Job | Universe | `WitnessRequest` | Tree | Counter row (expected; read at the task) |
|---|---|---|---|---|
| prove | both | `Roots({root})` | `Effects` | `DIGEST_ROW` (`cli_build_counters.rs:378`): program 1, bcg 1, cone 1, v2 1, scc 2, pd 1, ctx 1, snap 1, witness 1. Today prove is bcg 2, cone 2, scc 3, pd 2 (`:435-454`) |
| prove, no root resolved | both | `None` | none | as digest's no-root case: snap 0, witness 0 |
| fingerprint `--routine` | both | `Roots(pool)` | `Keep` | `DIGEST_ROW` |
| fingerprint `--inventory-only` | both | `Roots(∅)` | `Keep` | `DIGEST_ROW` with witness 0 |
| fingerprint B0, other query forms | single-app | none (the cut) | none | `FINGERPRINT_ROW` (`:491-501`), unchanged |
| `diff` | single-app | none (the cut) | none | unchanged |

Every subject is primary: cross-app, `witness.non_primary_members=0` must print on every run (E12,
`detector_context.rs:1381` `let non_primary = witness_non_primary_members(model, &outcome.raw);`).

### 4.7 D7: no `WitnessRequest` change; the seam with step 12

Step 9 uses only the existing `Roots(ids)` variant. It adds no variant and no field to `WitnessRequest`.
Step 12 (#277, running in parallel) changes `WitnessRequest` to the set form
`struct WitnessRequest { ids: BTreeSet<String>, primary: bool }` and replaces `needs_tree`/`tests` with
per-job tree demands (`TreeDemand { subjects, tests }`). **Step 12 lands the set form.** The seam:
- if step 9 merges first, step 12 rebases: `Roots(ids)` becomes `{ ids, primary: false }`, and step 9's
  `TreeUse` becomes a field of `TreeDemand` (`Effects` or `Keep`); `View::take_tree` becomes per job;
- if step 12 merges first, step 9 rebases: it writes `{ ids, primary: false }` where this spec writes
  `Roots(ids)`, and adds `TreeUse` to `TreeDemand` instead of to `Plan`.
- Both drafts keep the tree composed BEFORE `det` (this spec §4.5; step 12's D6.5), so the rebase does not
  change the order.

### 4.8 D8: the fail-closed trace (epic §11, open question 6)

On the tree, a cone that reaches a routine with no model row, or with no body, cannot clear prove:
1. The cone and the tree iterate model rows and the `rel.cone()` edges between them.
2. The ledger marks such calls: a resolved call into a body-less dependency row is `unfetched-dependency`
   (`let status = if into_unfetched {` … `"unfetched-dependency"`, `snapshot.rs:2599-2600`); a non-run call into
   a dependency object with no model row is `external` (`"external-target" => base_row("external", …)`,
   `:2636`); an object run is `unfetched-dependency` (`"opaque" => base_row("unfetched-dependency", …)`, `:2637`).
3. `unresolved_cone` harvests every row whose status is neither `resolved` nor `builtin`
   (`if row.status == "resolved" || row.status == "builtin" {` … `continue;`, `unresolved_cone.rs:152-154`).
4. Prove blocks on any harvested row (`obligations_clear`, `prove.rs:216`), and on any gap in the cone (§4.2
   item 5).

T1, T2 and T12 pin the three shapes. This table answers open question 6 (9A-8).

### 4.9 What is deleted

Prove's `compose_snapshot` import and call (`prove.rs:32`, `:888`), its `assemble_and_resolve_workspace_program`
call (`:873`) and its `AnalysisTarget::SingleApp` wrapper (`:880`). In fingerprint, the `--routine` and
inventory-only paths stop calling them; the code stays for B0 and the other query forms. Not deleted:
`compose_snapshot`, `build_r3a3_source_only_base`, `compose_full_snapshot` (B0, `diff`, the ordering cell, r4f,
tests).

## 5. Tests (every new test: break, failing assertion, revert; the discrimination-proof skill)

Fixtures: `tests/common/dep_dispatch_fixture.rs` (ws `CallChain` → dependency `Lib.Outer()` → `Inner`, a
Customer (18) write and a `Commit`), `tests/common/symbol_app` (an ABI-only app), the counters' `ws_dep()`. Each
test asserts its precondition before its claim.

| # | Test | Precondition (asserted) | Assertion | Break → fails |
|---|---|---|---|---|
| T1 (a) | `prove_two_hop_not_proven`: primary `Root` → dependency-source `A` → symbol-only `S.X`; `prove Root may-commit`, cross-app | `A` has a body; `X` has none; the `A→X` ledger row is `unfetched-dependency` (read with `digest --routine Root`) | `unknown`; blockers include `analysis-gap` and the `A→X` row | B1: prove's target forced to `SingleApp` → the blockers vanish, the answer is `no` (the false PROVEN; recorded). B2: `gaps_in_cone` forced to 0 → the gap blocker vanishes |
| T2 (a') | `prove_one_hop_ledger_and_gap`: `Root` → body-less `Y` of an app with source | the `Root→Y` row is `unfetched-dependency` | blockers are exactly `{unresolved-callsite at Root→Y, analysis-gap}` | B1: `into_unfetched` forced false → the callsite blocker is gone. B2: as T1-B2 |
| T3 (b) | `fingerprint_routine_inherits_through_dependency`: `fingerprint --routine CallChain --format json` | `--single-app` output has no fact on table 18 of the dependency | cross-app: an inherited `modify` on that table and a `COMMIT` on `CallChain`; every fact's subject is primary | B: the `--routine` path built single-app regardless of the flag → the facts vanish |
| T5 (d) | `fingerprint_selector_primary_only` | the cross-app identities hold a dependency `OnRun` and a primary `OnRun` | `--routine OnRun` renders the primary one, exit 0 | B: the selector index built with `None` → `SelectorAmbiguous`, exit 2 |
| T6 (d) | `fingerprint_root_pool_primary_only` (lib) | a tree with a dependency root classification row (built by the cross-app model, or hand-stated, F10) | no block for it; `total_classifications` counts primary rows only | B: drop the role filter → a block for the dependency routine appears |
| T7 (E9) | `no_dependency_parity_{prove,fingerprint_routine,inventory_only}` over every no-dependency r0 dir (201 of 216 have no `.alpackages` and no app.json dependency; recount at the task) | the dir has no dependency | cross-app output == `--single-app` output, byte for byte, except inventory's `analysisUniverse` | B1: append one cross-app-only diagnostic line → fails. B2: a class filter that drops one primary row → fails |
| T8 | `single_app_flag_reaches_{prove,fingerprint_routine,inventory_only}` | cross-app output differs from `--single-app` on the dependency fixture | `--single-app` output == the base's default output (cli-b goldens for prove; a golden minted at the base for the other two) | B: ignore the flag |
| T11 | `inventory_only_composes_no_raw` | — | `build.witness_pass` 0, `build.snapshot` 1; the output's four keys equal the same keys of `assemble_full_snapshot` over a `Roots(∅)` tree | B: give the plan `Roots(all primary)` → the counter is 1 |
| T12 | `abi_only_target_blocks_prove` | the row's status is `external` | `unknown`, blocked by `unresolved-callsite` | B: harvest `external` as resolved → `no` |
| T13 | `fingerprint_no_roots_config_cross_app` on `ws-d51-jobqueue` | its `roots.config.json` changes a root kind single-app | cross-app with `--no-roots-config`: `rootsConfigIgnored: true` and the unconfigured kinds, for `--routine` and inventory-only | B: `build_with` drops the parameter (literal `false`) |
| T14 | `cross_app_diagnostics_reach_prove_and_fingerprint`: `ws-codeunit-rec` with the 2.0.0.0 dependency mismatch (the `gate/diff/cli.rs:374` test's fixture) | `analyze`'s cross-app run reports the warning | prove's envelope `diagnostics` and `fingerprint --routine … --strict`'s lines carry it | B: read the diagnostics off a second, single-app view. If the warning is equal in both universes, the break cannot fail: record that T14 then pins the source by construction (one view per command) and say so as its limit |
| T15 | `prove_cone_truncation_blocks` (lib, a hand-stated chain of 66 routines) | depth > 64 | `coneTruncated`, `unknown` | B: ignore `cone_truncated` in `obligations_clear` |
| T16 | counters (§4.6) | — | the rows of §4.6, cross-app and `--single-app` | one break per changed row (e.g. prove left on the cut → bcg 2) |
| T17 (6A-9) | `prove_scoped_equals_unscoped`, `fingerprint_query_scoped_equals_unscoped` (lib; template `digest_rule_b_tests.rs:643` `fn t3_digest_scoped_equals_unscoped()`) | — | on the r0 dirs, the gap corpus and the dependency fixture, both universes: prove's entry with `Roots({root})` == with `Roots(all primary)`; the query's blocks with `Roots(pool)` == with `Roots(all primary)` | B: drop one subject from the pool → that block's inherited facts vanish |
| T18 | `full_snapshot_split_is_byte_identical` | — | `compose_full_snapshot` bytes (json, cbor, cbor.gz) at the head == the base's cli-b B0 and snapshot goldens (the existing differentials) | B: pass `assemble_full_snapshot` an empty permission array from B0 → `permissionFacts` differ |
| T19 | `prove_must_commit_through_boundary_is_unknown` | `Root` calls `Dep.X` with no guard; `Dep.X` commits | `must-commit` → `unknown` cross-app; `yes` `--single-app` is not asserted (single-app sees no commit) | B: let boundary occurrences into the MUST commit set → `yes` |
| T20 | `kept_tree_lifecycle` (lib, `session.rs`) | — | `Keep` leaves a tree that `take_tree` returns once; `release_all_but(&[])` drops an untaken tree; `take_tree` without `Keep` panics with its message; `Effects` behaviour unchanged | B: leave `Tree` out of `Cell::ALL` → the untaken tree survives `release_all_but` |
| T21 | `fingerprint_wide_query_stays_single_app` on `ws_dep()` | cross-app `--routine CallLib` differs from `--single-app` | `fingerprint` human with no flag, and `--roots <kind>` with no `--routine`, are byte-equal with and without `--single-app`, and `build.witness_pass` comes from the cut | B: route every query form to the new path → the outputs differ |

Rewritten with the code they pin (I2's exception): every `FingerprintOptions {` and `run_prove_pipeline(`
construction (field added); the counter rows; `cli_p1_inventory.rs:110`'s universe pin; prove's
`gaps_in_cone` unit tests (they move with `count_gaps_in_cone`).

## 6. False-positive and failure shapes

- **False PROVEN through opaque or body-less dependency code**: T1, T2, T12; F8.
- **False PROVEN by staying on the cut**: the cut walks workspace callers only. #220's T4c found the cut
  reaches 361 of 22,937 CDO dependency rows, so gaps reachable only through dependency code are missed.
  T1-B1 is that break.
- **False `yes` on a MUST question through a boundary**: T19.
- **False PROVEN by truncation**: T15. P4: the largest cross-app cone on CDO visits 19,284 of 25,000.
- **A scoped composition that drops a needed subject**: T17.
- **A selector or a root block on a dependency routine**: T5, T6.
- **The flag ignored, or the S′ boundary crossed**: T8, T21.
- **Diagnostics from the wrong universe**: T14 (with its stated limit).
- **`--no-roots-config` ignored cross-app**: T13.
- **A dependency subject with raw facts**: the census (E12).
- **The B0 or `diff` bytes moved by the split**: T18 and the cli-b differentials.
- **A kept tree leaked past the run**: T20 and the census's `view.live cells=` at output time.
- **A user reads cross-app and single-app forms of one command side by side.** `fingerprint --routine R`
  (cross-app) and `fingerprint` (single-app) can disagree on R's inherited facts. Neither form names its
  universe. This is the stated cost of S′, not a defect; the help text names it (§4.1) and the follow-up
  closes it.

## 7. Fallbacks (each pre-stated; each re-runs the identity gates it could affect)

| # | If | Then |
|---|---|---|
| F1 | a bounded row (B2, B1q, B1i) misses and the census at output time shows `Det`, `Caps`, `Rel`, `L4` or `Raw` alive | a defect: release before output; re-run Part 2 once |
| F2 | B1q or B1i still misses and the miss is the kept tree alive through `det` (the census shows `Tree` during the run) | read the query or the inventory inside `prepare`, right after `compose_tree` and before `det`, as digest reads its effects: `TreeUse::Keep` becomes `TreeUse::Fingerprint(FingerprintRead)`, whose output (the query result or the inventory map) lands in a cell and the tree is dropped there. T7, T8, T11, T17 re-run byte-identical |
| F3 | still missed after F2 | the form stays single-app by default in this step (the flag accepted and inert), the other forms ship; the miss and its numbers join the follow-up. No bound is relabelled |
| F6 | B2 (prove) misses after F1 | OD-2: read B2 against digest's B2′ (`≤ Pm(d) + 3,145,728 KiB`, `Pm(d)` the parent's digest `d` peak), stated in the numbers file |
| F7 | B2′ also misses | as F3 for prove |
| F8 | T1, T2, T12 or T19 answers PROVEN, or Part 1 CDO prove moves to PROVEN where single-app was `unknown` without a new primary effect | stop: E6 is violated by step 6's ledger, rule B or #220's gaps; file against them; prove stays on its base path |
| F9 | T7 or T8 differs on inventory-only (the tree path does not reproduce the cut's single-app inventory) | find the key that differs. If it is `coverage` or `identities` from the tree, single-app inventory keeps today's cut path (stated) and cross-app keeps the tree; else a defect in D4 |
| F10 | T6's precondition is not produced (no dependency root row cross-app) | hand-state the row in a lib test; the filter stays |
| F11 | T14's break cannot fail (the diagnostic is equal in both universes) | record T14's limit as in §5; no new diagnostic shape is invented |
| F12 | D (drift control, §8.1) fails twice | the B rows FAIL; nothing merges |
| F13 | a cross-app error-severity diagnostic on CDO | stated: `fingerprint --routine --strict` exits 1 there; the line is triaged |
| F14 | step 12 merges first | rebase per §4.7; T20 and T16 re-run |

## 8. Measurement plan

### 8.0 Feasibility (before any code)

Reference: `P_a` ≈ 3.14-3.16 GB (step 6 parent `o` 3,158,924 KiB; P4's cross-app digest `d` at the base,
3,137,152 KiB, sets the attach-level peak). Bound per command: `≤ 1.03 × P_a` ≈ 3.24-3.25 GB.

| Bound | Command (CDO) | Peak estimate | Reasoning | Feasible? |
|---|---|--:|---|---|
| B2 | `prove $WS R may-commit` | ≈ 3.14 GB | prove is digest with one root. Digest `d` (3 roots) at the base: 3,137,152 KiB (P4); `h` (88 roots): 3,216,736 KiB (P4). The peak is set while the cross-app model is built (single-app digest `d`: 743,192 KiB) | **yes**; margin ≈ 3 % |
| B1q | `fingerprint $WS --routine R --format json` | ≈ 3.14-3.3 GB (**estimate**) | `Roots({R})` as digest, plus the kept tree alive through `det`. The tree skeleton is not measured; if it is small against the attach peak, the peak does not move | **yes, unproven**; F2 is the pre-stated fallback |
| B1i | `fingerprint $WS --inventory-only` | ≈ 3.14-3.3 GB (**estimate**) | `Roots(∅)`: no raw facts; the skeleton tree alive through `det` | **yes, unproven**; F2 |
| B1s | `fingerprint $WS --format json` (B0, single-app) | unchanged ≈ 1.47 GB | path unchanged (step 6 bench: 1,471,224 KiB) | yes |
| B3s | `diff $WS $WS --format json` (single-app) | unchanged | path unchanged | yes |
| — | cross-app B0 / `diff` | ≥ 29.1 GB (B0, killed) | OD-1 | **no**: the follow-up |
| — | wide query forms cross-app | **estimate** ≥ 8 GB | 3,908 root classifications of 5,479 primary routines; policy's composition over all primary routines peaks at 8,327,444 KiB (step 6 B5) before any tree | **no**: they stay single-app (§2) |

### 8.1 Bounds (pre-stated; bands from this bench's parent runs; OD-4's void rule)

| Bound | Reading | Rule |
|---|---|---|
| D (drift control) | head `a` (cross-app `analyze`, default detectors, primary scope) vs parent `a` | `|H_a − P_a| ≤ 0.03 × P_a`; fails twice → F12 |
| B2 | head `pv` kernel peak, median of 3 | `H_pv ≤ 1.03 × P_a`; fallback F6 |
| B1q | head `fq` | `H_fq ≤ 1.03 × P_a`; fallback F2, F3 |
| B1i | head `fi` | `H_fi ≤ 1.03 × P_a`; fallback F2, F3 |
| B1s | head `fp` (B0 json, single-app) vs parent `fp` | `H_fp ≤ 1.03 × P_fp` (no regression) |
| B3s | head `df` (`diff $WS $WS`, single-app) vs parent `df` | `H_df ≤ 1.03 × P_df` (no regression) |
| ceiling | every head run | `≤ 12,582,912 KiB` (12 GiB). A probe or bench that could exceed it is not run in the shared cgroup |

`R` for `pv` and `fq` is the `d` root with the most visited routines (P4: 19,284), named in the bench script,
not here. Post-attach peaks are not readable on Linux (#204): "not measured" (9A-1).

### 8.2 Projection before the switch

None is needed: the bounded forms (B2, B1q, B1i) are feasible on estimate, and the infeasible forms are out of
scope. After Task 4 lands, the implementer runs `fq` once on CDO (cdo profile is enough for a peak, ≤ 4 GB
expected) as an early read of B1q. If it exceeds 1.03 × P_a by more than 5 %, F2 is applied before Task 5.

### 8.3 Bench parts (conductor, after the final panel)

`WS` from `.agent/config.json` (never written into a file). `P` = the base, `H` = the final-panel head;
`release-fast`; mimalloc as production; peaks by `maxrss.py`; OD-4's void rule.
- **Part 1 (identity and census)**: histogram SHA; analyze SARIF `cmp` (both scopes, both universes);
  single-app identity: parent default vs head `--single-app` for `pv`, `fq`, `fi`; parent vs head default for
  `fp` (json, cbor.gz) and `df`; the cross-app moves of `pv`, `fq`, `fi` saved and triaged per §3;
  `ALSEM_RETAINED_CENSUS=1` per cross-app command: `view.live cells=…` and `witness.non_primary_members=0`.
- **Part 2 (bounds)**: 3 rounds, interleaved: parent `a`, head `a`, head `pv`, `fq`, `fi`, parent and head
  `fp`, `df`.
- **Part 3 (reported)**: `bench_ab.py` parent vs head, same argv: `pv`, `fq`, `fi`, `fp`, `df`; sizes of `fq`,
  `fi` and `fp` json and `fp` cbor.gz.

### 8.4 Numbers file

`docs/measurements/2026-10-<dd>-issue-160-step9.md`, plus 9A-6's label fix.

## 9. Assumption probes (run 2026-10-24 at `b028ec32`)

Binary: `cargo build --profile cdo --bin alsem` in the worktree (a correctness profile: its peaks and counts
stand, its wall times are not quoted). Files: `/work/bench/160-9/a10-probe/`.

| probe | result | evidence |
|---|---|---|
| P0 drift | step 6's merge moved 22 files between the pre-draft's `581f3397` and `b028ec32` (among them `session.rs`, `fingerprint_cli.rs`, `fingerprint_query.rs`, `prove.rs`, `digest_cli.rs`, `snapshot.rs`); every cite in this spec was re-read at `b028ec32` | `git diff --stat 581f3397 b028ec32 -- src tests` |
| P1 diagnostics (move 1) | CDO digest `d` and `h`: **0** envelope diagnostics in both universes. Move 1 cannot show on CDO; T14 is the fixture | `p14-{d,h}-{x,s}.sum` |
| P4 caps (move 3) | cross-app: `d` 3 roots, max visited 19,284; `h` 88 roots, max 19,201; **0 truncated**. Single-app: max 354 / 366. Unresolved items summed over roots: `d` 839 cross-app / 154 single-app; `h` 15,995 / 2,953 | `p14-*.sum` |
| P4 peaks | cross-app digest `d` 3,137,152 KiB, `h` 3,216,736 KiB; single-app 743,192 / 736,700 KiB | `p14-*.err` (`maxrss:` lines) |
| P15 prove single-app == digest single-app | for all 18 non-dummy cli-b prove cases, the prove golden's `evidence` equals digest `--single-app --routine R`'s effects with the same `factId`s, and both report 0 unresolved items. Limit: only effects the golden cites are compared, and no case has an unresolved item or a gap; the cli-b prove goldens are the real gate | `p15.txt`, `p15-*.digest.json`, `cases.tsv` |
| P5 dependency root rows | static: `derive_root_classifications` (`snapshot.rs:2866`) iterates `resolved.root_classifications` with no role filter, so a cross-app tree holds whatever the cross-app model classifies. Whether CDO's cross-app model classifies dependency routines was not run | source; T6 hand-states the row (F10) |
| P7 no-dependency r0 dirs | 216 dirs; 12 hold `.alpackages`, 13 declare an app.json dependency, 9 have no app.json; 201 have neither a package folder nor a declared dependency | shell count over `tests/r0-corpus/*/` |
| P8 roots config | only `tests/r0-corpus/ws-d51-jobqueue/roots.config.json` in the repo | `ls` |
| P9 counters | prove today: program 1, bcg 2, cone 2, v2 1, scc 3, pd 2, ctx 1, snap 1, witness 1 (`cli_build_counters.rs:435-454`); `FINGERPRINT_ROW` the same (`:491-501`); `DIGEST_ROW` bcg 1, cone 1, scc 2, pd 1 (`:378-388`); the counters' `ws_dep()` already has a dependency | source |
| P13 fail-closed shapes | holds at the base: `unfetched-dependency` for a resolved call into a body-less dependency row and for an object run, `external` for a call with no model row; `unresolved_cone` harvests everything but `resolved`/`builtin` | `snapshot.rs:2599-2600`, `:2636-2637`; `unresolved_cone.rs:152-154` |
| P16 `routine_to_stable` | the cut base's map is built by `routine_to_stable_of(ws)`, so D4's split can read it off the model | `capability_cone.rs:4555`, `:4647` |
| P17 permissions | `derive_permissions` reads `base.coverage`, `base.direct_full` and `base.raw` (every primary routine's raw facts), so the inventory tree path cannot produce it without a full primary composition | `snapshot_full.rs:912-951` |
| P18 cli-b prove fixtures | every cli-b prove fixture has an `app.json` (the dummy-doc case runs on `ws-d8-commit-in-tx`), so the cross-app default builds on all of them | `cli_b_prove_differential.rs` case list; `ls` |

Not run: P2/P3 cross-app B0 and `diff` (done for OD-1; out of scope now); P10 (the parent analyze band: the
bench's own parent rounds give it); P6 (selector ambiguity on CDO: T5 hand-states it); any probe above 8 GB.

## 10. Amendments this spec asks the epic to take

- **9A-1** post-attach peaks "not measured" on Linux (#204).
- **9A-2** of the three output moves, (1) and (3) are delivered here; (2) dependency shards is a B0 form and
  moves to the follow-up.
- **9A-3** `fingerprint --inventory-only` composes no raw facts (`Roots(∅)`). Cross-app, it is the inventory of
  a tree with no inherited facts and no permission facts (§4.4 item 3).
- **9A-5** `--no-roots-config` reaches the cross-app build (`build_with`).
- **9A-6** the 10+11a numbers file's "cross-app fingerprint" row is single-app.
- **9A-8** open question 6 is answered by §4.8.
- **9A-9** rule B reaches prove and the fingerprint query (OD-3).
- **9A-10** row 9 is delivered for prove, the `--routine` query and inventory-only (bounds as worded).
  Fingerprint B0, the wide query forms and `diff` cross-app, with fixtures (c), the dependency-shard move and
  the cli-b `cross-app` header, move to the follow-up (OD-1 = S′), which starts from OD-1's numbers and must
  not assume the subject-keyed fold helps (97.65 % kept).

## 11. Dependencies

- **Step 6** (merged, `b028ec32`): `WitnessRequest`, `Cell::{Caps, Raw, Tree, Effects}`, the composing
  `prepare`, digest on the tree, rule B, `DigestEffects`.
- **Step 12** (#277, parallel): the seam in §4.7. Step 12 lands the `WitnessRequest` set form.
- **#161** (not a dependency): the ordering cell.
- **#204** (not a dependency): post-attach peaks.
- **The follow-up** (filed from the ledger): fingerprint B0, the wide query forms and `diff` cross-app.
