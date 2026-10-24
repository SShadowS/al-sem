# #160 step 9: prove and the fingerprint query cross-app (scope S′)

Status: spec, attempt 10, 2026-10-24, revised after spec panel round `9 r1` (register entries S9-*, O9-*).
Cited at `b028ec32` (master with step 6 merged, #274). The branch was rebased onto `317d3b34` (#278) and then
`1134bed1` (#237); neither touches a file cited here, so every cite holds there too.

Child of `2026-10-18-issue-160-analysis-session-design.md` (the epic): §9 row **9**; E6, E7, E9, E12, E18;
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
  (`session.rs:89` `pub enum WitnessRequest {`). Its routines are the **subjects** of the composition.
- **Tree**: a `CapabilitySnapshot` built by `compose_tree` (`snapshot.rs:1374` `pub(crate) fn compose_tree(`)
  over the view's cone graph (`rel.cone()`).
- **Tree read**: what a command computes off the tree inside `prepare`, before `det` is built, after which the
  tree is dropped. Digest's tree read is `digest_effects`.
- **Cut**: the old single-app snapshot, `compose_snapshot` over `build_r3a3_source_only_base` (workspace
  callers only). The cut is what prove and fingerprint read today.
- **B0**: fingerprint's full-snapshot path (json with no query flag, cbor, cbor.gz, `--shard`).
- **Query path**: fingerprint's json-with-query-flags and human forms (`fingerprint_query`).
- **Boundary group**: a rule-B effect: the effects a primary routine reaches only through one dependency
  routine, grouped per (boundary, effect type) (step 6 §14.3.5).
- **PROVEN**: a prove answer that is not `unknown`. **False PROVEN**: a PROVEN answer the program does not
  support (E6).
- **Rule B**: step 6 §14.3.5. A witness stops at the first dependency routine; boundary groups carry
  `conditionality: unknown`. Every tree carries it (`boundary: Some(boundary)` in `compose_tree`); the cut never
  does.
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
2. **`--single-app`** on `prove` and `fingerprint` restores the base's output byte for byte on these forms,
   except the one stated fix of §4.3 (`mayCommit` over inherited coverage), which moves both universes.
3. **The query path filters by role** (epic Decision 28): a selector never resolves to a dependency routine,
   and `totalClassifications` counts primary root classifications only.
4. **No false PROVEN and no false `no` cross-app** (E6): a cone that enters body-less, ABI-only or opaque
   dependency code blocks the answer, and a dependency effect hidden inside a boundary group still matches a
   question about it.
5. **Prove and the in-scope query forms stop reading the cut.** After this step `compose_snapshot` has no
   reader in `prove.rs`, and the `--routine` query path and inventory-only do not call it.

## 2. Non-goals

- **Fingerprint B0 and `diff` stay single-app** (OD-1 = S′). Their code path is unchanged. So are the query
  forms with no `--routine` (human with no flag, `--roots` alone, `--witness` or `--no-include-inherited`
  alone): they render every root classification (3,908 on CDO, §8.0), which costs about as much as B0's
  composition. They move with B0 in the follow-up.
- `diff` gains no `--single-app` flag in this step: the flag would be inert. The follow-up adds it.
- No change to resolution, demand, the cone, coverage, roles, key classes, the DAG witness rule or rule B
  (steps 5, 6, 8 own them). Histogram SHA and analyze SARIF are unchanged. **One gap-rule fix is in scope**
  (D9, §4.10): it removes a contradiction between #220's gap derivation and its own assertion.
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
| A9.1 | the cross-app `View` for the commands | prove: §4.2, T1, T2, T8. Query `--routine`: §4.3, T3, T8. Inventory-only: §4.4, T8, T11. Fingerprint B0 and `diff`: **not deliverable in this step, because** OD-1 measured B0 at ≥ 29.1 GB (killed); follow-up filed. If F3, F7 or F8 fires for a form, that form's row reads as §7's alternate contract states |
| A9.2 | `--single-app` | T8 (dependency fixture), T23 (the corpus-wide parent-versus-head sweep), §8.3 Part 1 (CDO) |
| A9.3 | three stated output moves | (1) diagnostics source: **pinned by construction, no discriminating fixture**: each command reads its diagnostics off the one view that answers it (§4.2 item 3, §4.3 step 3). No diagnostic is known that only a cross-app build emits; P1 shows 0 lines on CDO in both universes. T14 pins the CHANNEL only. (2) dependency shards: **not deliverable**, shards are a B0 form (follow-up). (3) `unresolved_cone` caps: unchanged by this step; "unchanged" rests on P4's distribution (max 19,284 of 25,000; 0 truncated). T15 pins that a truncation reaches prove's answer end to end and that the depth cap of 64 is what truncates its chain |
| A9.4 | fixtures (a), (a'), (b), (c), (d) | (a) T1; (a') T2; (b) T3 on the query path; (d) T5, T6. (c) dependency-mediated `capability-gained` is a `diff` fixture: **not deliverable**, follow-up |
| A9.5 | one discrimination per filter | role filters: T5, T6. Universe switches: T1-B1, T3-B, T8, T21. Gap count: T1-B2, T2-B2. Boundary-group matching: T24. Rule B in the query: T25. Rule B in prove: T19 |
| A9.6 | no-dependency parity (E9) | T7 over every r0 dir with an `app.json` and no dependency; T22 pins that the dirs with no `app.json` keep today's error, unchanged, in both universes |
| A9.7 | CDO fingerprint time and size reported | §8.3 Part 3: `fq`, `fi` and single-app B0 time and size. Cross-app B0: the OD-1 lower bound (≥ 29.1 GB, killed, no output) is the report |
| A9.8 | both peaks ≤ P_a + band | §8.1: B2 (prove), B1q (`--routine` query), B1i (inventory-only) as worded. B0 and `diff`: no-regression bounds B1s, B3s (they stay single-app) |
| A9.9 | cell census | §8.3 Part 1: the new output-time census line (§4.5) and `witness.non_primary_members=0` per cross-app run |
| A9.10 | a miss walks §5.12's ladder | §7 |
| A9.11 | goldens: cli-b header `cross-app`; contents unchanged | cli-b prove, fingerprint and diff goldens: **byte-identical** except the stated `mayCommit` fix (§4.3), which may move query goldens only (triaged per §3's move table). The `cross-app` header in cli-b B0 files moves to the follow-up |
| E6 | never falsely PROVEN | T1, T2, T12, T15, T19, T24; §6 |
| E7 | behavioural fingerprint/diff cross-app | the query half: T3, T25, T26. The B0 and `diff` half: follow-up |
| E9 | parity | T7 |
| E12 | no dependency subject materialized | every cross-app run in Part 1 prints `witness.non_primary_members=0` |
| E18 | the producer arena never alive during a run | holds: every tree, with its `pd_solution`, is dropped inside `prepare` before `det` (§4.5). T20 (c) pins it |
| 6A-9 | prove and fingerprint arms of scoped-equals-unscoped | T17 |
| E14 / I10 | CDO ratchets | `scripts/cdo-gate` |

### Expected output moves, and how each is triaged

| Where | Move | Triage |
|---|---|---|
| cli-b goldens (prove, digest, diff, snapshot, fingerprint B0) | none | byte-compare; any move fails |
| cli-b fingerprint query goldens (human, and the `ERROR_SELECTOR` query) | only `mayCommit` `no` → `unknown` where the routine's inherited coverage is not complete (§4.3 item 6) | every moved line is that field on a routine whose `coverage.inheritedStatus` is not `complete`; anything else fails |
| `tests/cli/cli_build_counters.rs` rows | prove and the `--routine` query lose the standalone composition; inventory-only composes no raw facts (§4.6) | each row read at the base |
| the existing tests listed in §5 "Rewritten with the code they pin" | they now run on the cross-app path | each is named with the universe it pins |
| prove, `fingerprint --routine`, `--inventory-only` on a workspace with no `app.json` | **none**: with no `app.json` the input set has no model-instance id (`compute_model_instance_id` returns `None` at `let app_id = self.app_id()?;`, `inputs.rs:443-444`), so both today's pipelines (`prove.rs:868-871`, `fingerprint_cli.rs:368-372`) and `AnalysisTarget::build` (`run.rs:185-188`) fail first with `"{command}: could not compute modelInstanceId"`, exit 1, in either universe; the cross-app `NoPrimaryApp` arm is never reached | T22 pins the unchanged message |
| CDO prove, cross-app | single-app `unknown` can become `yes` (a matching effect, a boundary group included) or `no` (the dependency code is fully analysed). A may-question `yes` cannot drop: cross-app only adds effects. A MUST `yes` (`commits-on-success-path`) can drop to `unknown` | an `unknown` → `yes` must cite an evidence effect (a boundary group counts); every `unknown` → `no` is triaged against the dependency source (the E6 check). A `yes` → `no` is a defect (F8) |
| CDO `fingerprint --routine R` | inherited dependency facts appear on R; witnesses end at the boundary | reported; a primary fact lost is a defect |
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
  one-line wrappers passing `false`. The 11 `AnalysisTarget::build(` and 18 `build_cross_app_base(` call
  sites are untouched.
- `ProveCli` (`alsem.rs:533` `struct ProveCli {`) and `FingerprintCli` (`alsem.rs:559` `struct FingerprintCli {`)
  gain `--single-app`, with digest's help text (`alsem.rs:661` `/// Analyse the workspace alone. The default
  loads the dependencies it requires`) plus, for fingerprint, one sentence: "Applies to `--routine` and
  `--inventory-only`; the other forms analyse the workspace alone in this release."
- `FingerprintOptions` gains `single_app: bool`; `run_prove_pipeline` (`prove.rs:840`) gains `single_app: bool`.
  Every construction is updated (17 `FingerprintOptions {` sites; 5 `run_prove_pipeline(` and 2
  `prove_pipeline_with(` call sites). A site that means today's behaviour passes `true`.
- Failure texts are `AnalysisTarget::build`'s: `"{command}: could not compute modelInstanceId"`,
  `"{command}: workspace did not resolve"`, and `cross_app_failure_message` (`run.rs:206`), whose head already
  names `--single-app`. A workspace with no `app.json` has no model-instance id, so it fails at
  `"{command}: could not compute modelInstanceId"` (`run.rs:185-188`) before the cross-app arm, exactly as
  prove and fingerprint fail today (§3 move table: no change). Exit codes are unchanged.

### 4.2 D2: prove is digest with one root

`prove_pipeline_with` (`prove.rs:859`) today builds a single-app view
(`let mut view = View::new(AnalysisTarget::SingleApp(Box::new(resolved)));`, `:880`), runs a no-root
`digest_plan` (`:881`), then composes the cut (`let mut snap = compose_snapshot(resolved);`, `:888`). It
becomes digest's pipeline (`digest_cli.rs:1724-1800`) with one selector, in both universes:

1. `AnalysisTarget::build(inputs, single_app, "prove")`, `View::new(target)`.
2. The root is resolved before `prepare`, in a block that ends before it: `derive_identity_table(model)`,
   `OwnerIndex::build(model)`, `resolve_changed_roots(&identities, model, &changed_input,
   primary_guid_of(model), &owners)` (digest's block, `digest_cli.rs:1761-1771`).
3. `digest_plan(detectors, {roots[0]}, false)`: prove reads only the first resolved root (`prove.rs:947-948`),
   so the request holds that one id and the counters and census match the stated subject. `prepare`;
   `finish_default_run`; `view.take_effects()` when `plan.needs_tree()`. The diagnostics are the run's, off the
   same view.
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
6. **Boundary groups keep queryable resources (S9-1).** A boundary group's `detail` lists `boundary`,
   `resourceCount` and at most the first 10 `resources`, and no `resourceId`/`resourceDisplay`
   (`digest.rs:4064-4078`, `.take(10)`). Prove's `effect_matches` (`prove.rs:375`) reads
   `resource_name_matches(&eff.detail, table)`, so `writes-table:T` and `publishes-event:E` would miss a
   dependency-only effect and could answer a false `no`. Fix: `DigestEffectResult` (and `DigestEffectFull`)
   gains a non-serialized `group_resources: Vec<(String, String)>`, the group's FULL (resource id, display)
   set, filled where `detail` is built. `effect_matches` on a boundary group tests the target against every
   member with the same rule `resource_name_matches` uses (display, or the id's trailing segment,
   case-insensitive). Digest's JSON is unchanged (the field is not serialized). T24 pins it with a target that
   is the 11th resource.
7. Everything else after (`DbTempStates` off `view.l4()`, `run_digest_query_full_from_entries`, `prove(…)`'s
   question logic, the projection) is unchanged.

Single-app, the tree is over `rel.cone()`, which equals the cut's edges (step 6 P13), and digest single-app
already runs this path. P15 (§9) found every cli-b prove golden's evidence equal to digest single-app's
effects for the same routine; T23 extends that to a corpus-wide parent-versus-head byte sweep. Cross-app, rule
B applies (OD-3): a dependency effect is a boundary group with `conditionality: unknown`
(`digest.rs:3928-3930`), so it never satisfies `commits-on-success-path`, which needs an `UNCONDITIONAL`
COMMIT (`prove.rs:463-468`).

### 4.3 D3: the fingerprint query with `--routine`

**Which forms.** `run_fingerprint_pipeline` (`fingerprint_cli.rs:358`) routes to the query path when no B0
condition holds (`let go_b0 = opts.shard.is_some() || …`, `:416-419`). In that path, the query is **in scope**
when `!opts.routine_selectors.is_empty()`. In-scope forms take the new path in both universes. Every other
query form keeps today's path (single-app, the cut) whatever `--single-app` says (§2), apart from item 6's fix.

**The new path:**
1. `AnalysisTarget::build_with(opts.inputs, opts.single_app, opts.no_roots_config, "fingerprint")`,
   `View::new(target)`.
2. The selectors are resolved before `prepare`, from the model alone: `derive_identity_table(model)`, the
   selector index built with the primary app guid (Decision 28, below), `resolve_selector` and
   `narrow_to_owner` as `fingerprint_query` does (`fingerprint_query.rs:663-664`). The set of ids that resolve
   to exactly one routine is the pool. A failed selector adds nothing; its diagnostic comes from step 3.
3. The plan is `fingerprint_query_plan(detectors, pool, filters, roots_config_ignored)`: `fingerprint_plan`'s
   run plus `WitnessRequest::Roots(pool)` and the tree read `TreeRead::FingerprintQuery` (§4.5). Inside
   `prepare`, right after `compose_tree` and before `det`, the read runs
   `fingerprint_query(&tree, &filters, roots_config_ignored, &OwnerIndex::build(model), roles)` and keeps its
   `FingerprintQueryResult` in a cell; the tree is dropped there. After `prepare`: the run's analyzer
   diagnostics (`analyzer_diags_for(&view, &key)`, as today, `fingerprint_cli.rs:391`),
   `view.take_tree_read()`, `release_all_but(&[])`. `fingerprint_query` re-resolves the selectors over
   `tree.identities`, the same `derive_identity_table` output, so the diagnostics (unresolved, ambiguous) are
   unchanged.
4. **The result is self-contained (r2 item 1).** Today the projection still reads the snapshot after the query:
   `project_fingerprint_query_full(&result, &snap, …)` builds its operation index from `snap.operation_index`
   for each witness's terminal evidence (`fingerprint_query.rs:925-946`, used at `:801-820`), and the human
   selector-error path builds the #239 owner hint from `snap.identities` (`fingerprint_cli.rs:538`). With the
   tree dropped in `prepare`, neither exists. So:
   - `BlockWitness` gains `evidence: Option<FactEvidence>` and each `ProjectedPath` gains
     `boundary: Option<PathBoundary>` (item 5), owned values computed inside `fingerprint_query` while the tree
     is alive. `project_fingerprint_query_full` drops its `snap` parameter and writes them; on the cut no path
     has a boundary and every fact's evidence is today's operation evidence from the same operation index, so
     the cut path's bytes cannot move (T18's differentials and T23 check it);
   - the #239 hint on the new path builds its selector index from `derive_identity_table(view.model())`,
     called after `prepare` and only on the error path; it is the same table the tree's `identities` holds
     (step 2). The cut path keeps `snap.identities`.
   The `--strict` gate stays before any output, as today (`fingerprint_cli.rs:398-413`). The rest of the
   projection (from `let snap = compose_snapshot(resolved);`, `fingerprint_cli.rs:499`, on) is unchanged apart
   from these two reads.
5. **Rule B reaches the query (S9-2).** `build_fingerprint_indexes_pub` drops the boundary metadata, and the
   adapter `pub_idx_as_private` sets `boundary: None` with the comment "The fingerprint query reads the cut:
   rule B is off." (`digest.rs:5156-5158`). `FingerprintIndexesPub` (`digest.rs:5024`) gains
   `boundary: Option<&'a BoundaryRule>` and `boundary_displays`, filled from `snap.boundary` the way the private
   index fills them; the adapter passes them through. **The boundary belongs to each path (S9-2, O9-24).**
   Under rule B one inherited fact can be reached through several boundaries (two call sites into `Lib.Outer`;
   `Outer1` and `Outer2` both reaching `Inner`), so the boundary is carried per path, never per fact:
   - **`PathBoundary`** is filled from what `boundary_terminal` (`digest.rs:1481`) already computes for the
     path's last edge, with step 6's identity rule (`digest.rs:1497-1537`), one form per rule-B boundary kind:
     `Call { id: "cs:<callsite>", callsite_id, anchor }` (the anchor from `callsite_index`);
     `Trigger { id: "op:<operation>", operation_id, anchor }` (an implicit-trigger edge: the anchor from
     `operation_index`, else the edge's own anchor); `Event { id: "ev:<event>", event_id, anchor }` (an
     event-dispatch edge: the publish site's anchor, `publish_anchor`). The JSON writes it on the path object
     as `"boundary": { "anchor": …, "id": …, "kind": "call"|"trigger"|"event", "callsiteId"|"operationId"|"eventId": … }`,
     only when present, so a path on the cut keeps today's bytes. The human path line names the same boundary
     (today's `terminal_hop`).
   - **The fact's evidence** (`BlockWitness.evidence`, today's `evidence` object) is chosen deterministically:
     the boundary of the FIRST projected path that has one, in the paths' existing order (step 6 made that order
     deterministic); when no path has a boundary, or `paths` is empty (the `Some(0)` witness limit,
     `fingerprint_query.rs:565-576`, or a capped truncation), it is today's operation evidence. So a fact with
     paths never shows an operation inside dependency code as its evidence when its witness stops at a boundary.
   - T25 pins the call, trigger and event kinds, two boundaries on one fact, and the empty-paths fallback.
6. **`mayCommit` reads the coverage the query asked for (S9-3).** Today `presence` is `no` when no commit fact
   is rendered and `coverage.direct_status == "complete"` (`fingerprint_query.rs:449-457`), even when inherited
   facts are rendered. For `Root → source dependency A → opaque X`, Root's direct coverage is complete and its
   inherited coverage is not, so `mayCommit: no` is false. Fix: when `filters.include_inherited`, read
   `coverage.inherited_status`; else `direct_status`. This is a root-cause fix in shared code, so it also moves
   the single-app and wide forms wherever inherited coverage is incomplete; those moves are stated in §3,
   triaged field by field, and listed under CHANGELOG `Fixed`. T26 pins it.

**Decision 28 (role).** Two filters, each pinned at the CLI:
- the selector index takes `primary_guid_of(model)` instead of `None` in both places that build it
  (`build_selector_indexes(&snap.identities, None)`, `fingerprint_query.rs:655`, and the human error path,
  `fingerprint_cli.rs:538`), as digest and prove pass it (T5);
- `totalClassifications` counts the classifications whose routine is `Role::Primary`, **before** the
  selector and `--roots` filters (today `let total_classifications = snap.root_classifications.len();`,
  `fingerprint_query.rs:675`, is also taken before them). The role is read from the model row
  (`Model::routine_role`), not a guid prefix. Single-app every row is primary, so nothing moves. Under
  `--routine` the pool is already primary-only once the selector index is, so this count is the filter's
  one production effect (P5: the cross-app model does classify dependency routines). T6 pins it at the CLI.

`fingerprint_query` takes the roles through a new parameter (`primary_guid: Option<&str>` and an
`is_primary: &dyn Fn(&str) -> bool` over stable ids), so the cut path passes the single-app values.

### 4.4 D4: `--inventory-only` on a tree with no raw facts

Today inventory-only goes down B0: `compose_full_snapshot(resolved, &full_opts)` (`fingerprint_cli.rs:438`),
then `build_inventory_envelope(&tree, …)` (`:467`). So it composes the whole cut snapshot and every primary
routine's raw facts, then reads five keys (`analysisUniverse`, `apps`, `identities`, `coverage`,
`rootClassifications`, `snapshot_full.rs:1336` `pub fn build_inventory_envelope(`, `:1474-1478`).

1. **Split `compose_full_snapshot`** (`snapshot_full.rs:137`) into the cut half and an assembly half:
   - `compose_full_snapshot(resolved, opts)` keeps its signature and its 8 callers (B0, `diff`, tests): it
     builds `let base = build_r3a3_source_only_base(resolved);` (`:163`), `core`, and
     `derive_permissions(resolved, &base)` (`:174`), then calls the assembly half.
   - `assemble_full_snapshot(resolved, core, permission_facts, opts, keys)` is everything after: contracts,
     schema, `reachable_gaps`, `filter_by_class`, `project_apps`, and the CBOR map in its literal key order.
     `keys` is `All` for B0 and `Inventory` for inventory-only, which converts only the five keys
     `build_inventory_envelope` reads, `analysisUniverse`, `apps`, `identities`, `coverage` and
     `rootClassifications` (`snapshot_full.rs:1474-1478`; a missing key becomes `Null`), to `CborValue`; the
     other keys would be built and thrown away.
     `reachable_gaps` (`:507` `fn reachable_gaps(`) and `filter_by_class` read only `routine_to_stable` from the
     cut base; both take `routine_to_stable_of(ws)` (`capability_cone.rs:4555`) instead, the one builder the cut
     base's own field is made with (`:4647` `let routine_to_stable = routine_to_stable_of(ws);`). So B0 and
     `diff` bytes cannot move (T18).
2. **Inventory-only** builds the target with `build_with(.., opts.single_app, opts.no_roots_config, ..)`, the
   plan `fingerprint_inventory_plan(detectors, opts)` = `fingerprint_plan`'s run plus `WitnessRequest::Roots(∅)`
   and the tree read `TreeRead::Inventory`. Inside `prepare`, right after `compose_tree`, the read calls
   `assemble_full_snapshot(model, tree, CborValue::Array(vec![]), opts, Inventory)` with
   `universe: SnapshotUniverse::of(target)` and keeps the five-key map; the tree is dropped there. After
   `prepare`, `build_inventory_envelope` runs unchanged over that map. `Roots(∅)` composes no raw facts (digest
   `--tests`' shape, `session.rs:166-170` doc), so `build.witness_pass` stays 0 (`capability_cone.rs:3221-3222`:
   the counter moves only when `raw` is not empty).
3. **What the inventory says cross-app** (a stated definition, 9A-3): the inventory projection of a tree whose
   raw scope is empty. `permissionFacts` is empty because it needs every primary routine's raw facts
   (`derive_permissions` reads `base.raw`, `snapshot_full.rs:950-951`), and the inventory does not print it. It
   is passed to `filter_by_class` only as a source of references, so a dependency identity or app referenced
   ONLY by a permission fact or an inherited fact is not kept. Every primary row is kept (`filter_by_class`
   keeps every primary id, step (5)).
4. **Single-app** takes the same tree path. T7 (no-dependency parity), T8 and T23 (`--single-app` == the base's
   bytes) compare it with today's cut path; F9 is the fallback if they differ.

### 4.5 D5: tree reads in the `View`

Today a tree exists only for digest: `assert!(!plan.needs_tree || matches!(plan.witness,
WitnessRequest::Roots(_)), "a tree is composed over a Roots witness request");` (`session.rs:407-410`), and it
dies inside `prepare` after `digest_effects` (`session.rs:540-547`).

**Why the tree is read inside `prepare`, not kept (O9-10).** A kept tree would live through `det` and the run.
Its size was measured (P19, §9): the cross-app tree of digest `d` holds **296,115,964 B (282.4 MiB)** of
census buckets, of which `pd_solution` is 201,311,977 B (exact) and `capability_facts` only 22,679,160 B
(shallow). So a `--routine` or inventory tree is ≈ 270-280 MiB whatever its raw scope (the skeleton, without
`capability_facts`, is 260.8 MiB, and the shallow buckets undercount owned strings), against a band of
≈ 95 MB (`0.03 × P_a`). Keeping it through `det` would very likely miss B1q and B1i. So every tree is read and
dropped inside `prepare`, before `det`, as digest's is; this keeps E18 (the producer arena is never alive
during a run) without an amendment.

- `Plan.needs_tree: bool` (`session.rs:81`) becomes `tree: TreeRead`, `enum TreeRead { None, Effects,
  FingerprintQuery(QueryRead), Inventory(InventoryRead) }`. `Effects` is today's digest behaviour.
  `QueryRead` holds the filters, `roots_config_ignored` and the roles; `InventoryRead` the snapshot options
  `assemble_full_snapshot` needs. `Plan::needs_tree()` (`:281`) stays, as `tree == TreeRead::Effects`, for
  digest's one reader.
- In `prepare`, the tree block runs for every read other than `None`, in the same place and order as today
  (after the composition, before `det` takes `caps`), and calls the read's function on the tree. The
  dependency calls are taken in the same first prepare (`let dependency_calls = match &mut *target {`,
  `session.rs:468-473`; step 6's F1-C2: a second prepare cannot get them back), with the filter
  `.filter(|_| plan.tree != TreeRead::None)`.
- The two new results live in one new cell, `Cell::TreeRead`, holding an `enum TreeReadOut {
  FingerprintQuery(FingerprintQueryResult), Inventory(IndexMap<String, CborValue>) }`; both are owned and need
  nothing from the tree afterwards (§4.3 item 4, §4.4 item 2). `View::take_tree_read()`
  moves it out and marks the cell released; it panics with `"no tree read: the plan asked for none"`
  otherwise. `Cell::ALL` is unchanged: `release_all_but` drops a `TreeRead` cell only when it is filled, so
  no command that never reads a tree gains a `view.release` trace line (O9-5; the sequences pinned by
  `views_release_their_cells_before_the_next_build`, `cli_build_counters.rs:984-1070`, stay).
- **The release mask widens (r2 item 3).** `Cell::TreeRead` is a ninth cell, but the mask is a `u8`
  (`released: u8`, `session.rs:53`; `fn bit(self) -> u8`, `:144-145`; `fn note_release(released: &mut u8, …`,
  `:332`). The three become `u16`. T20 (f) asserts that every `Cell` variant's bit is distinct and non-zero.
- The assertion keeps its rule: a tree (any read) is composed over a `Roots` request.
- **Output-time census (S9-10, O9-15).** Today `[retained-census] view.live cells=…` prints once, before the
  run (`session.rs:613-614`), so it cannot show what is alive at serialization. New: `View::census_output()`
  prints `[retained-census] view.output cells=<filled cells> rel_strong=<Arc::strong_count of the relation,
  the target's included> tree=<0|1>` when `ALSEM_RETAINED_CENSUS` is set. Prove, the `--routine` query and
  inventory-only call it right before they serialize. F1 reads this line. T20 checks the fields directly.

### 4.6 D6: subjects and counters per command (I1, I5)

| Job | Universe | `WitnessRequest` | Tree read | Counter row (expected; read at the task) |
|---|---|---|---|---|
| prove | both | `Roots({roots[0]})` | `Effects` | `DIGEST_ROW` (`cli_build_counters.rs:378`): program 1, bcg 1, cone 1, v2 1, scc 2, pd 1, ctx 1, snap 1, witness 1. Today prove is bcg 2, cone 2, scc 3, pd 2 (`:435-454`) |
| prove, no root resolved | both | `None` | none | as digest's no-root case: snap 0, witness 0 |
| fingerprint `--routine` | both | `Roots(pool)` | `FingerprintQuery` | `DIGEST_ROW` |
| fingerprint `--inventory-only` | both | `Roots(∅)` | `Inventory` | `DIGEST_ROW` with witness 0 |
| fingerprint B0, other query forms | single-app | none (the cut) | none | `FINGERPRINT_ROW` (`:491-501`), unchanged |
| `diff` | single-app | none (the cut) | none | unchanged |

Every subject is primary: cross-app, `witness.non_primary_members=0` must print on every run (E12,
`detector_context.rs:1381` `let non_primary = witness_non_primary_members(model, &outcome.raw);`).

### 4.7 D7: no `WitnessRequest` change; the seam with step 12

Step 9 uses only the existing `Roots(ids)` variant, `Roots(∅)` included. It adds no variant and no field to
`WitnessRequest`. Step 12 (#277, running in parallel) changes `WitnessRequest` to the set form
`struct WitnessRequest { ids: BTreeSet<String>, primary: bool }`, where empty means today's `None`, and
replaces `needs_tree`/`tests` with per-job tree demands (`TreeDemand { subjects, tests }`). **Step 12 lands the
set form.** Rules both sides keep, whichever merges second:
1. `Roots(ids)` maps to `{ ids, primary: false }`.
2. **A tree demand forces the composition whatever its request's size (O9-11).** Today `need_raw` is
   `plan.witness != WitnessRequest::None` and `need_caps` follows from it (`session.rs:396-398`), so digest
   `--tests` and step 9's inventory (`Roots(∅)` + a tree read) compose only because `Roots(∅) != None`. Under
   the set form an empty request means `None`, so `need_caps` must also be true when any job demands a tree.
3. Step 9's `TreeRead` becomes a field of `TreeDemand` (`Effects`, `FingerprintQuery`, `Inventory`); each
   result is per job. Each read's output is OWNED and needs nothing from the tree afterwards (§4.3 item 4: the
   query result carries each witness's terminal evidence; the inventory map carries its five keys), so a batch
   may drop each job's tree right after its read, before `det`.
3a. The release mask is `u16` after this step (§4.5); a step 12 cell added later takes the next free bit.
4. **A job's tree read is computed over that job's own subjects (S9-12).** In a batch the view's composition
   may hold other jobs' raw facts, and those facts change which dependency identities and apps
   `filter_by_class` keeps (`snapshot_full/key_classes.rs:806-869`). Step 12's D5.2 already gives each tree job
   its own tree over its own subjects; inventory's subjects are ∅, so its tree must hold no inherited fact
   even when another job's do. Step 12 must add a standalone-versus-batch byte test over a batch of
   inventory-only, two `--routine` queries on different roots, and prove.
5. Step 12's D5.1 assumption "fingerprint and diff = `{∅, true}`" no longer holds: step 9 uses `Roots(pool)`
   and `Roots(∅)`, and `diff` stays single-app.
6. Both drafts read the tree BEFORE `det` (this spec §4.5; step 12's D6.5).
7. Step 12's D6.1 rewrites the view's releases; step 9 adds no `Cell::ALL` member, so the two do not collide
   there (§4.5).

### 4.8 D8: the fail-closed trace (epic §11, open question 6)

On the tree, a cone that reaches a routine with no model row, or with no body, cannot clear prove:
1. The cone and the tree iterate model rows and the `rel.cone()` edges between them.
2. The ledger marks such calls: a resolved call into a body-less dependency row is `unfetched-dependency`
   (`let status = if into_unfetched {` … `"unfetched-dependency"`, `snapshot.rs:2599-2600`); a non-run call into
   a dependency object with no model row is `external` (`"external-target" => base_row("external", …)`,
   `:2636`); an object run is `unfetched-dependency` (`"opaque" => base_row("unfetched-dependency", …)`, `:2637`).
3. `unresolved_cone` harvests every row whose status is neither `resolved` nor `builtin`
   (`if row.status == "resolved" || row.status == "builtin" {` … `continue;`, `unresolved_cone.rs:152-154`).
4. A body-less row has direct coverage `unknown` with reason `opaque-dependency` (`capability_cone.rs:524-530`),
   and an `external-target` call is a coverage reason (`capability_cone.rs:4293`), so the caller's inherited
   coverage is `partial`, and prove adds `coverage-incomplete` (`prove.rs:278-284`).
5. Prove blocks on any harvested row, any gap in the cone (§4.2 item 5) and incomplete coverage
   (`obligations_clear`, `prove.rs:216-223`).

T1, T2 and T12 pin the three shapes. This table answers open question 6 (9A-8).

### 4.9 What is deleted, and the docs that change

Prove's `compose_snapshot` import and call (`prove.rs:32`, `:888`), its `assemble_and_resolve_workspace_program`
call (`:873`) and its `AnalysisTarget::SingleApp` wrapper (`:880`). In fingerprint, the `--routine` and
inventory-only paths stop calling them; the code stays for B0 and the other query forms. Not deleted:
`compose_snapshot`, `build_r3a3_source_only_base`, `compose_full_snapshot` (B0, `diff`, the ordering cell, r4f,
tests). Doc comments updated with the code (O9-17): `compose_snapshot`'s readers (`snapshot.rs:1270-1276`:
fingerprint B0, the wide query forms, `diff`); the comment above `derive_analysis_gaps` that names "`prove`'s
`count_gaps_in_cone`" (now in `digest_cli.rs`); `digest_plan`'s doc ("prove, which keeps its standalone
snapshot", `session.rs:162-170`); the "fingerprint query reads the cut" comment (`digest.rs:5156`).

### 4.10 D9: a source app's body-less row is not a symbol-only boundary (O9-3)

`derive_analysis_gaps` puts the app of ANY body-less, non-parse-incomplete dependency row into `opaque_apps`
and emits one `symbol-only-boundary` gap per such app; its later `body-unavailable` loop skips every row whose
app is in `opaque_apps`, so it never emits for a dependency row. But #220's `reachable_gaps` asserts
`debug_assert!(… "a symbol-only-boundary gap names an app with source")` (`snapshot_full.rs:536-545`, F21: "an
opaque boundary is a source-less app"). Today only the unshipped cross-app B0 path reaches that assertion;
step 9's inventory path does. So a cross-app inventory of a workspace whose source dependency has a body-less
row (T2's fixture shape) panics in a debug build, and the gap's label is wrong. Fix the derivation, the side
the assertion's contract names: an app joins `opaque_apps` only when the cross-app base says it has no source
(`DependencyApp::has_source == false`); a body-less row of an app with source gets the per-routine
`body-unavailable` gap the third loop already emits. Both gap kinds block prove (`count_gaps_in_cone` matches a
routine id or an app prefix), so no answer can move to PROVEN. T27 pins it.

**Where the set comes from (r2 item 6).** `derive_analysis_gaps` (`snapshot.rs:2661`) is shared by the cut
(`compose_snapshot_from_base`, `:1310`) and the tree (`compose_tree`, `:1419`). It takes the set through
`TreeInputs` as `source_less_apps: Option<&HashSet<String>>`:
- `compose_tree` passes `Some`, built from the view's cross-app base (`dependency_apps`, beside `app_names`);
  single-app it passes `Some(∅)`, and a single-app model has no dependency rows, so nothing moves;
- `compose_full_snapshot` with `SnapshotUniverse::CrossApp(u)` passes `Some`, from `u.base.dependency_apps`;
- every other cut caller (`compose_snapshot(resolved)`: the ordering cell, r4f, B0 and `diff` single-app,
  tests) passes `None`, which keeps today's rule (every body-less dependency row's app), so their bytes cannot
  move.
`tests/cli/cli_snapshot_schema4.rs`'s cross-app snapshot tests stay green unchanged: their symbol-only apps
(the R5B pair, `ZDeep`) have no source, so they keep their `symbol-only-boundary` gaps (`:221-262`, `:459-490`),
and `parse_error_body_is_not_symbol_only` (`:512`) already asserts the source app gets no app-level gap.

## 5. Tests (every new test: break, failing assertion, revert; the discrimination-proof skill)

Fixtures: `tests/common/dep_dispatch_fixture.rs` (ws `CallChain` → dependency `Lib.Outer()` → `Inner`, a
Customer (18) write and a `Commit`), `tests/common/symbol_app` (an ABI-only app), the counters' `ws_dep()`.
Each test asserts its precondition before its claim. A break that fails at a different assertion than the one
named is recorded as such.

| # | Test | Precondition (asserted) | Assertion | Break → fails |
|---|---|---|---|---|
| T1 (a) | `prove_two_hop_not_proven`: primary `Root` → dependency-source `A` → symbol-only `S.X`; `prove Root may-commit`, cross-app | `A` has a body and no COMMIT in its cone (else may-commit is `yes` before any obligation, `prove.rs:414-421`); `X` has none; the `A→X` ledger row is `unfetched-dependency` (read with `digest --routine Root`) | `unknown`; `blockedBy` contains `analysis-gap`, `coverage-incomplete` and the `A→X` callsite | B1: pass `&[]` as the tree's dependency calls in `prepare` → the `A→X` row leaves the ledger and the callsite assertion fails. B2: `gaps_in_cone` forced to 0 → the `analysis-gap` assertion fails |
| T2 (a') | `prove_one_hop_ledger_and_gap`: `Root` → body-less `Y` of an app with source | the `Root→Y` row is `unfetched-dependency`; the tree's gap for `Y` is `body-unavailable` (D9) | blockers are exactly `{unresolved-callsite at Root→Y, analysis-gap, coverage-incomplete}` | B1: `into_unfetched` forced false (`snapshot.rs:2599`) → the callsite blocker is gone. B2: as T1-B2 |
| T3 (b) | `fingerprint_routine_inherits_through_dependency`: `fingerprint --routine CallChain --format json` | `--single-app` output has no fact on table 18 of the dependency | cross-app: an inherited `modify` on that table and a `COMMIT` on `CallChain`; every fact's subject is primary | B: the `--routine` path built single-app regardless of the flag → the facts vanish |
| T5 (d) | `fingerprint_selector_primary_only` (CLI) | the cross-app identities hold a dependency `OnRun` trigger (reached by `Codeunit.Run`) and a primary `OnRun` | `--routine OnRun` renders the primary one, exit 0 | B: the selector index built with `None` → `SelectorAmbiguous`, exit 2 |
| T6 (d) | `fingerprint_total_counts_primary_roots` (CLI, T5's fixture) | the cross-app model classifies the dependency `OnRun` (P5; asserted through the view's model) | `totalClassifications` equals the primary root classifications, counted before the selector | B: count every classification → the total is one higher |
| T7 (E9) | `no_dependency_parity_{prove,fingerprint_routine,inventory_only}` over every r0 dir with an `app.json` and no dependency (192 of 216; recount at the task) | the dir has an `app.json`, no `.alpackages`, no declared dependency | cross-app output == `--single-app` output, byte for byte, except inventory's `analysisUniverse` | B1: append one cross-app-only diagnostic line → fails. B2: a class filter that drops one primary row → fails |
| T8 | `single_app_flag_reaches_{prove,fingerprint_routine,inventory_only}` | cross-app output differs from `--single-app` on the dependency fixture | `--single-app` output == the base's default output (a golden minted from the base binary) | B: ignore the flag |
| T11 | `inventory_only_composes_no_raw` | — | `build.witness_pass` 0, `build.snapshot` 1; the output's five keys equal the same keys of `assemble_full_snapshot` over a `Roots(∅)` tree | B: give the plan `Roots(all primary)` → the counter is 1 |
| T12 | `abi_only_target_blocks_prove` | the row's status is `external`; Root's coverage reasons include `external-target` | `unknown`, blockers include `unresolved-callsite` and `coverage-incomplete` | B: skip `external` rows in the harvest (`unresolved_cone.rs:152`) → the `unresolved-callsite` assertion fails (the answer stays `unknown` through `coverage-incomplete`; recorded) |
| T13 | `fingerprint_no_roots_config_cross_app` on `ws-d51-jobqueue` | its `roots.config.json` gives a classification a `configEntryId` | cross-app with `--no-roots-config`: the `--routine` query reports `rootsConfigIgnored: true` and the unconfigured kinds; inventory-only has no classification with a `configEntryId` (the inventory carries no `inputsMetadata`) | B: `build_with` drops the parameter (literal `false`) |
| T14 | `diagnostics_channel_prove_and_fingerprint` (`ws-codeunit-rec` with the 2.0.0.0 dependency mismatch, the `gate/diff/cli.rs:374` test's fixture) | the run reports the version warning | prove's envelope `diagnostics` and the `fingerprint --routine … --format json` envelope's diagnostics carry it (a warning does not fail `--strict`, and the json query embeds diagnostics rather than printing them, `fingerprint_cli.rs:593-610`) | B: drop the run's diagnostics from the new path's envelope → fails. Not a universe proof (A9.3 (1)) |
| T15 | `prove_cone_truncation_blocks` (CLI, a generated workspace with a 66-routine call chain) | the chain is longer than `DEFAULT_MAX_DEPTH` (asserted against the constant) | `obligations.coneTruncated: true`, `unknown`, a `cone-truncated` blocker | B: `cone_truncated: false` in place of `entry.unresolved_traversal.truncated` (`prove.rs:212`, the production transfer) → fails |
| T16 | counters (§4.6) | — | the rows of §4.6, cross-app and `--single-app` | one break per changed row (e.g. prove left on the cut → bcg 2) |
| T17 (6A-9) | `prove_scoped_equals_unscoped`, `fingerprint_query_scoped_equals_unscoped` (lib; template `digest_rule_b_tests.rs:643` `fn t3_digest_scoped_equals_unscoped()`) | — | on the r0 dirs, the gap corpus and the dependency fixture, both universes: prove's entry with `Roots({root})` == with `Roots(all primary)`; the query's blocks with `Roots(pool)` == with `Roots(all primary)` | B: drop one subject from the pool → that block's inherited facts vanish |
| T18 | `full_snapshot_split_is_byte_identical` | — | `compose_full_snapshot` bytes (json, cbor, cbor.gz) at the head == the base's cli-b B0 and snapshot goldens (the existing differentials) | B: pass `assemble_full_snapshot` an empty permission array from B0 → `permissionFacts` differ |
| T19 | `prove_commits_on_success_path_through_boundary_is_unknown` | `Root` calls `Dep.X` with no guard; `Dep.X` commits with no guard; no gap in the cone; `coverageComplete: true` (all asserted) | `prove Root commits-on-success-path` → `unknown` cross-app | B: give boundary groups their computed path conditionality instead of `UNKNOWN` (`digest.rs:3928-3930`) → `yes` |
| T20 | `tree_read_lifecycle` (lib, `session.rs`) | — | (a) a `FingerprintQuery` read leaves a result that `take_tree_read` returns once; (b) a filled, untaken `TreeRead` cell is dropped by `release_all_but(&[])` (field checked directly); (c) after `prepare` with any tree read, the `tree` field is `None` before `det` runs (E18); (d) `take_tree_read` after an `Effects` plan panics with its message; (e) `Effects` behaviour unchanged; (f) every `Cell` variant's `bit()` is non-zero and distinct (the `u16` mask) | B1: skip the tree drop after a `FingerprintQuery` read → (c) fails. B2: give `Cell::TreeRead` the bit of `Cell::Det` → (f) fails |
| T21 | `fingerprint_wide_query_stays_single_app` on `ws_dep()` | cross-app `--routine CallLib` differs from `--single-app` | human with no flag, and `--roots <kind>` with no `--routine`, are byte-equal with and without `--single-app`, and the trace's `build.bcg` is 2 (`FINGERPRINT_ROW`, the cut) | B: route every query form to the new path → bcg 1 and the outputs differ |
| T22 | `no_app_json_keeps_its_error` (the 9 r0 dirs with no `app.json`) | the dir has no `app.json`; the base binary prints `"{command}: could not compute modelInstanceId"`, exit 1 | prove, `fingerprint --routine` and `--inventory-only` print that same line and exit 1, with and without `--single-app` | B: make the new path skip the model-id check (build the target from a dummy id) → the line differs |
| T23 | `single_app_sweep` (ledger record, not a committed test) | the base binary is built from the parent | for every r0 dir with an `app.json`: `fingerprint --routine <each root> --format json` and human, `--inventory-only`, and `prove <each root> <each question>` on the gap corpus and every cli-b case: base default == head `--single-app`, byte for byte, apart from §4.3 item 6's field | any other difference is a defect, fixed before the final panel |
| T24 | `prove_matches_a_group_resource_beyond_ten` (CLI) | `Root` reaches, only through `Dep.W`, writes to 11 dependency tables; the target sorts 11th by resource id; the group's `detail.resources` lists 10 | `prove Root writes-table:<11th>` → `yes`, evidence = the group | B: match a group on `detail` only → `unknown` or `no` |
| T25 | `fingerprint_witness_stops_at_boundary` (CLI, the dependency fixture) | the inherited Table 18 fact's carrier is the dependency routine `Inner` | five arms, each asserting its precondition: (call) the witness ends at `Lib.Outer`, never `Inner`; the path's `boundary` is `kind: call`, `id: cs:<CallChain→Outer>`, anchor = that callsite's `callsiteIndex` row; the fact's `evidence` is that boundary, with no `operationId` from inside `Inner`. (trigger) a primary insert fires a dependency table trigger that writes: the path's `boundary` is `kind: trigger`, `id: op:<insert op>`. (event) a primary publisher whose dependency subscriber writes: `kind: event`, `id: ev:<event>`, anchor = the publish site. (two call sites) one fact reached through two calls into `Lib.Outer`: two paths, each with its own `cs:` boundary; the fact's `evidence` is the first path's, and two runs are byte-equal. (`--witness 0`) `paths` is empty and `evidence` is today's operation evidence | B1: keep the adapter's `boundary: None` → the call arm reaches `Inner`. B2: one boundary per fact (the first path's copied to every path) → the two-call-site arm fails. B3: fill `PathBoundary` for call-site edges only → the trigger and event arms fail. B4: pick the fact's evidence from an unordered set (iterate a `HashMap` of boundaries) → the two-call-site arm's evidence or byte-equality fails (run it 5 times). B5: write no evidence when `paths` is empty → the `--witness 0` arm fails |
| T26 | `fingerprint_may_commit_uses_inherited_coverage` (CLI) | `Root → source dependency A → opaque X`; Root's direct coverage `complete`, inherited not complete; no commit fact rendered | `mayCommit: unknown` with inherited facts; `no` with `--no-include-inherited` | B: read `direct_status` always → `no` |
| T27 | `source_app_bodyless_row_gets_body_unavailable_gap` (lib, D9) | a source dependency app with one body-less demanded row | the tree's gaps hold `body-unavailable` for that routine and no `symbol-only-boundary` for the app; cross-app inventory of that workspace does not panic in a debug build | B: put every body-less row's app back into `opaque_apps` → the gap assertion fails (and the debug assertion fires) |

**Rewritten with the code they pin (I2's exception)**, each named with the universe it now pins: every
`FingerprintOptions {` and `run_prove_pipeline(` construction (the field added); the counter rows
(`cli_build_counters.rs:504-536`: `fingerprint_query` and `fingerprint_inventory_only` move to cross-app on
`ws_dep()`, `fingerprint_json` stays single-app); `cli_p1_inventory.rs:110` (default is now `"cross-app"`; a
`--single-app` twin keeps `"single-app"`) and `:116-123` (the byte equality now compares a cross-app inventory
with a single-app B0; it holds because ws-d8 has no dependency, and the test says so); `cli_s6_consumers.rs:100-131`
(`--routine Run6`, now cross-app); `cli_analyze_call_hierarchy_json.rs:385-392` (`--inventory-only`, now
cross-app); `cli_b_fingerprint_differential.rs:425` (the `ERROR_SELECTOR` query, now cross-app); prove's
`count_gaps_in_cone` unit tests (they move with the function).

## 6. False-positive and failure shapes

- **False PROVEN through opaque or body-less dependency code**: T1, T2, T12; F8.
- **What staying on the cut costs (corrected, O9-1).** Single-app, a call into a dependency is a to-less
  `ExternalTarget` (`program_calls.rs:73-77`): its ledger row is `external` and an `external-target` coverage
  reason, so the cut answers `unknown`, not a false PROVEN. The cut's cost is precision, not soundness: it
  cannot answer `yes` from a dependency effect or `no` when the dependency code is fully analysed. T1-B1 is a
  break of the cross-app ledger, not of the universe.
- **False `no` through a boundary group's resource list**: T24.
- **False `yes` on a MUST question through a boundary**: T19.
- **False `mayCommit: no` across an opaque dependency**: T26.
- **False PROVEN by truncation**: T15. P4: the largest cross-app cone on CDO visits 19,284 of 25,000.
- **A scoped composition that drops a needed subject**: T17.
- **A selector or a root count on a dependency routine**: T5, T6.
- **A witness that walks into dependency code**: T25.
- **The flag ignored, or the S′ boundary crossed**: T8, T21, T23.
- **Diagnostics in the wrong channel**: T14 (the universe of the source is pinned by construction only).
- **`--no-roots-config` ignored cross-app**: T13.
- **A dependency subject with raw facts**: the census (E12).
- **The B0 or `diff` bytes moved by the split**: T18 and the cli-b differentials.
- **A tree alive past `prepare`**: T20 (c) and the output-time census.
- **A debug panic on a source app's body-less row**: T27.
- **A user reads cross-app and single-app forms of one command side by side.** `fingerprint --routine R`
  (cross-app) and `fingerprint` (single-app) can disagree on R's inherited facts. Neither form names its
  universe. This is the stated cost of S′, not a defect; the help text names it (§4.1) and the follow-up
  closes it.

## 7. Fallbacks (each pre-stated; each re-runs the identity gates it could affect)

| # | If | Then |
|---|---|---|
| F1 | a bounded row (B2, B1q, B1i) misses and the output-time census (§4.5) shows `Det`, `Caps`, `Rel`, `L4` or `Raw` filled, `rel_strong` > 1 beyond the target's own, or `tree=1` | a defect: release before output; re-run Part 2 once |
| F2 | B1q still misses with a clean census | the query read is trimmed: `fingerprint_query`'s fact, edge and witness indexes are built over the pool's cones only, while the GLOBAL inputs stay whole: `tree.identities` (selector re-resolution) and `tree.root_classifications` (the primary `totalClassifications`). T5, T6, T7, T8, T17, T23 re-run byte-identical. **B1i has no F2**: its five keys are global by definition (every primary routine's inventory row, coverage and classification, and the identities `filter_by_class` keeps from every serialized reference), so nothing can be trimmed without changing them; a B1i miss with a clean census goes straight to F3 |
| F3 | B1q still missed after F2, or B1i missed after F1 | that form stays single-app by default in this step, under the alternate contract below |
| F6 | B2 (prove) misses after F1 | OD-2: read B2 against digest's B2′ (`≤ Pm(d) + 3,145,728 KiB`). `Pm(d)` is measured then: three interleaved `release-fast` runs of the PARENT's cross-app digest `d`, alongside the head's `pv`, in the same bench slot; never a historical or `cdo`-profile number. Stated in the numbers file |
| F7 | B2′ also misses | as F3 for prove |
| F8 | T1, T2, T12 or T19 answers PROVEN (each must stay `unknown`); T24 answers anything but its expected `yes` with the group as evidence; or in Part 1 a CDO prove answer moves `yes` → `no`, or `unknown` → `yes` with no cited evidence effect, or `unknown` → `no` and the dependency source triage shows an effect the answer misses | stop: E6 is violated by step 6's ledger, rule B, #220's gaps or this step's matching; file against the owner; prove stays on its base path for this step under the alternate contract below |
| F9 | T7, T8 or T23 differs on inventory-only (the tree does not reproduce the cut's inventory on a no-dependency dir) | a defect: find the key that differs and fix the tree side if the cut is right. If not closed in this attempt, inventory-only stays on today's cut path in BOTH universes (so T7 and T8 hold) under the alternate contract below |
| F10 | T6's precondition is not produced (the cross-app model classifies no dependency `OnRun`) | P5 says it does; if the fixture does not, the filter has no production effect and is deleted with T6, and §4.3 says so |
| F12 | D (drift control, §8.1) fails twice | the B rows FAIL; nothing merges |
| F13 | a cross-app error-severity diagnostic on CDO | stated: `fingerprint --routine --strict` exits 1 there; the line is triaged |
| F14 | step 12 merges first | rebase per §4.7 rules 1-7; T16, T20 re-run, and step 12's mixed-job test (rule 4) must exist |

**The alternate contract when F3, F7, F8 or F9 fires for a form (S9-13).** That form keeps today's code path
in both universes and its default is single-app. `--single-app` stays accepted on it and is inert. Its help
sentence drops it from the "Applies to" list; the CHANGELOG lists it under "stays single-app"; A9.1 and A9.8
read "not deliverable for <form>, because <fallback, number>", and the number joins the follow-up issue's
body. Its cross-app-only tests (T1, T2, T3, T24, T25 for the form concerned) are deleted from the branch and
their text is copied into the follow-up; its T8 becomes "output equal with and without `--single-app`"; its
T7, T16 and T23 rows revert to the base's. The other forms ship as specified.

## 8. Measurement plan

### 8.0 Feasibility (before any code)

Reference: `P_a` ≈ 3.14-3.16 GB (step 6 parent `o` 3,158,924 KiB; P4's cross-app digest `d` at the base,
3,137,152 KiB). Bound per command: `≤ 1.03 × P_a` ≈ 3.24-3.25 GB; the band is ≈ 95 MB.

| Bound | Command (CDO) | Peak estimate | Reasoning | Feasible? |
|---|---|--:|---|---|
| B2 | `prove $WS R may-commit` | ≈ 3.14 GB | prove is digest with one root. Digest `d` (3 roots) at the base: 3,137,152 KiB (P4, P19); `h` (88 roots): 3,216,736 KiB; `--tests` (a tree, its read before `det`): 3,169,488 KiB (P19). The tree is dropped before `det` | **yes**; margin ≈ 3 % |
| B1q | `fingerprint $WS --routine R --format json` | ≈ 3.14-3.2 GB (**estimate**) | digest's shape: `Roots({R})`, the tree (≈ 282 MiB, P19) read and dropped before `det`, as digest's. The query result for one root is small | **yes** on estimate; F2, F3 |
| B1i | `fingerprint $WS --inventory-only` | ≈ 3.14-3.3 GB (**estimate**) | `Roots(∅)`; the read builds four CBOR keys over the tree before `det` (identities 7.2 MB, coverage 3.8 MB, root classifications 3.1 MB shallow, P19) | **yes** on estimate; F2, F3 |
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
| B1i | head `fi` | `H_fi ≤ 1.03 × P_a`; fallback F3 (no F2: §7) |
| B1s | head `fp` (B0 json, single-app) vs parent `fp` | `H_fp ≤ 1.03 × P_fp` (no regression) |
| B3s | head `df` (`diff $WS $WS`, single-app) vs parent `df` | `H_df ≤ 1.03 × P_df` (no regression) |
| ceiling | every head run | `≤ 12,582,912 KiB` (12 GiB). A probe or bench that could exceed it is not run in the shared cgroup |

`R` for `pv` and `fq` is the `d` root with the most visited routines (P4: 19,284), named in the bench script,
not here. Post-attach peaks are not readable on Linux (#204): "not measured" (9A-1).

### 8.2 Early read

The bounded forms are feasible on estimate and the infeasible forms are out of scope, so no projection gates
the work. One early read: after plan Task 3 (the `--routine` query) lands and before Task 4 (inventory) starts,
the implementer runs `fq` once on CDO with the `cdo` profile, peak by `maxrss.py`. If it exceeds
`1.08 × P_a` (the bound plus 5 %, since the `cdo` profile is not the bench's), F2 is applied before Task 4.

### 8.3 Bench parts (conductor, after the final panel)

`WS` from `.agent/config.json` (never written into a file). `P` = the base, `H` = the final-panel head;
`release-fast`; mimalloc as production; peaks by `maxrss.py`; OD-4's void rule.
- **Part 1 (identity and census)**: histogram SHA; analyze SARIF `cmp` (both scopes, both universes);
  single-app identity: parent default vs head `--single-app` for `pv`, `fq`, `fi` (apart from §4.3 item 6's
  field, triaged); parent vs head default for `fp` (json, cbor.gz) and `df`; the cross-app moves of `pv`, `fq`,
  `fi` saved and triaged per §3; `ALSEM_RETAINED_CENSUS=1` per cross-app command: the `view.output` line and
  `witness.non_primary_members=0`.
- **Part 2 (bounds)**: 3 rounds, interleaved: parent `a`, head `a`, head `pv`, `fq`, `fi`, parent and head
  `fp`, `df`; plus parent digest `d` if F6 fires.
- **Part 3 (reported)**: `bench_ab.py` parent vs head, same argv: `pv`, `fq`, `fi`, `fp`, `df`; sizes of `fq`,
  `fi` and `fp` json and `fp` cbor.gz.

### 8.4 Numbers file

`docs/measurements/2026-10-<dd>-issue-160-step9.md`, plus 9A-6's label fix.

## 9. Assumption probes (run 2026-10-24 at `b028ec32`)

Binary: `cargo build --profile cdo --bin alsem` in the worktree (a correctness profile: its peaks and counts
stand, its wall times are not quoted). Files: `/work/bench/160-9/a10-probe/`.

| probe | result | evidence |
|---|---|---|
| P0 drift | step 6's merge moved 22 files between the pre-draft's `581f3397` and `b028ec32`; every cite was re-read at `b028ec32` | `git diff --stat 581f3397 b028ec32 -- src tests` |
| P1 diagnostics (move 1) | CDO digest `d` and `h`: **0** envelope diagnostics in both universes | `p14-{d,h}-{x,s}.sum` |
| P4 caps (move 3) | cross-app: `d` 3 roots, max visited 19,284; `h` 88 roots, max 19,201; **0 truncated**. Single-app: max 354 / 366. Unresolved items summed over roots: `d` 839 cross-app / 154 single-app; `h` 15,995 / 2,953 | `p14-*.sum` |
| P4 peaks | cross-app digest `d` 3,137,152 KiB, `h` 3,216,736 KiB; single-app 743,192 / 736,700 KiB | `p14-*.err` |
| P15 prove single-app == digest single-app | for all 18 non-dummy cli-b prove cases, the golden's `evidence` equals digest `--single-app --routine R`'s effects with the same `factId`s; both report 0 unresolved items. Limit: only cited effects; no case has an unresolved item or a gap (T23 covers more) | `p15.txt`, `cases.tsv` |
| P5 dependency root rows | **yes**: `classify_roots` (`root_classification.rs:290`) classifies every model row and is called on the whole cross-app `ModelEntities` (`workspace.rs:1920-1921`), with no role filter; a demanded dependency trigger (table, page, codeunit `OnRun`) or subscriber is classified. Dynamic confirmation is T6's precondition | source |
| P7 no-dependency r0 dirs | 216 dirs; 12 hold `.alpackages`, 13 declare a dependency, 9 have no `app.json`; 201 have no dependency, of which 192 have an `app.json` (T7) and 9 do not (T22) | shell count |
| P8 roots config | only `tests/r0-corpus/ws-d51-jobqueue/roots.config.json` | `ls` |
| P9 counters | prove today: program 1, bcg 2, cone 2, v2 1, scc 3, pd 2, ctx 1, snap 1, witness 1 (`cli_build_counters.rs:435-454`); `FINGERPRINT_ROW` the same; `DIGEST_ROW` bcg 1, cone 1, scc 2, pd 1 | source |
| P13 fail-closed shapes | holds | `snapshot.rs:2599-2600`, `:2636-2637`; `unresolved_cone.rs:152-154` |
| P16 `routine_to_stable` | the cut base's map is built by `routine_to_stable_of(ws)` | `capability_cone.rs:4555`, `:4647` |
| P17 permissions | `derive_permissions` reads `base.coverage`, `base.direct_full` and `base.raw` | `snapshot_full.rs:912-951` |
| P18 cli-b prove fixtures | every one has an `app.json` and no dependency | case list; `ls` |
| P19 tree size (O9-10) | `ALSEM_RETAINED_CENSUS=1` on CDO digest `d`: the tree's census buckets sum to 296,115,964 B (282.4 MiB) cross-app (`pd_solution` 201,311,977 B exact; `typed_edges` 16.6 MB, `callsite_resolutions` 16.3 MB, `capability_facts` 22.7 MB, `routine_order_frames` 10.7 MB, `callsite_index` 9.0 MB, `identities` 7.2 MB, shallow) and 14,488,230 B (13.8 MiB) single-app. `--tests` gives the same tree. Peaks: `d` cross-app 3,137,052 KiB, single-app 732,472 KiB, `--tests` 3,169,488 KiB | `census-{d-x,d-s,dt-x}.err` (run by a `sonnet` agent) |

Not run: P2/P3 cross-app B0 and `diff` (done for OD-1; out of scope now); P10 (the bench's own parent rounds
give the band); P6 (T5 states it); any probe above 8 GB.

## 10. Amendments this spec asks the epic to take

- **9A-1** post-attach peaks "not measured" on Linux (#204).
- **9A-2** of the three output moves, (3) is delivered here (unchanged caps, P4), (1) is pinned by
  construction only (no diagnostic is known that only a cross-app build emits), and (2) dependency shards is a
  B0 form and moves to the follow-up.
- **9A-3** `fingerprint --inventory-only` composes no raw facts (`Roots(∅)`). Cross-app, it is the inventory of
  a tree with no inherited facts and no permission facts (§4.4 item 3).
- **9A-5** `--no-roots-config` reaches the cross-app build (`build_with`).
- **9A-6** the 10+11a numbers file's "cross-app fingerprint" row is single-app.
- **9A-8** open question 6 is answered by §4.8.
- **9A-9** rule B reaches prove and the fingerprint query (OD-3), including the query's witnesses (§4.3 item 5).
- **9A-10** row 9 is delivered for prove, the `--routine` query and inventory-only (bounds as worded).
  Fingerprint B0, the wide query forms and `diff` cross-app, with fixture (c), the dependency-shard move and
  the cli-b `cross-app` header, move to the follow-up (OD-1 = S′), which starts from OD-1's numbers and must
  not assume the subject-keyed fold helps (97.65 % kept).
- **9A-11** #220's gap derivation is corrected to its own F21 rule (§4.10): `symbol-only-boundary` names only
  an app with no source.
- **9A-12 (E18, S9-15)** no amendment is needed: every tree, with its producer arena, is read and dropped
  inside `prepare` before `det` (§4.5), so E18 holds as worded. Recorded so a later step that keeps a tree
  through a run knows it needs one.

## 11. Dependencies

- **Step 6** (merged, `b028ec32`): `WitnessRequest`, `Cell::{Caps, Raw, Tree, Effects}`, the composing
  `prepare`, digest on the tree, rule B, `DigestEffects`.
- **Step 12** (#277, parallel): the seam in §4.7. Step 12 lands the `WitnessRequest` set form and keeps rules
  2 and 4 of §4.7.
- **#161** (not a dependency): the ordering cell.
- **#204** (not a dependency): post-attach peaks.
- **The follow-up** (filed from the ledger): fingerprint B0, the wide query forms and `diff` cross-app.
