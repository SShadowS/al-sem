# Issue #220 — #160 step 8: roles, snapshot schema 4, preflight

## Claim

```json
{"run_id":"20261022-220347-220d76","issue":220,"attempt":1,"session":"claude-code-session:e2bf58b3-5d1b-467f-8c3b-1c59a8d42f0c","branch":"issue/220-160-step-8-roles-snapshot-schem-a1","worktree":"/work/al-sem-issue-220-a1","body_hash":"5f714f19bd78dbb06a104accd28d8ddd8a1902de89a04160c1673d354dee50fe","title":"#160 step 8: roles, snapshot schema 4, preflight","reconciled":[]}
```

Issue body hash: `5f714f19bd78dbb06a104accd28d8ddd8a1902de89a04160c1673d354dee50fe` (claim `body_hash`).
Before the claim the conductor edited the issue body: `#160` → `epic 160` in the `## Dependencies and ordering` section only, because the executor's dependency parser read it as a hard dependency (the text says "Depends on nothing unmerged"). The operator approved the edit through the permission prompt.

## Classification

**architectural.** A schema version bump (snapshot 3 → 4) with a preflight change, one role rule replacing about 13 sites across l4/l5/gate/model, live gap derivation with reachability, and key classes. It touches several subsystems, and every cli-b golden moves. There is a pre-draft at `/work/bench/160-8/draft-step8-design.md`. It assumed steps 5 and 6 had merged first; they have not (the operator split this step out to merge BEFORE step 6). The spec rebases the design on the real base.

## Assumption probes

Run 2026-10-22 on the claim base `f96619ec` (only 11b merged; 5 and 6 not). Outputs under
`/work/bench/160-8/`. Binaries: `target/release-fast/alsem` built in the worktree (copied to
`/work/bench/160-8/alsem-base`; the probe build to `alsem-probe`). The probe patch is saved as
`/work/bench/160-8/probe.patch` and was reverted; `git diff --stat -- src crates tests` printed
nothing afterwards. CDO = the pinned CDO workspace. No timing below is a bound.

| probe | result | evidence |
|---|---|---|
| P1 site census | **holds, plus one more rule.** 153 regex hits. Role sites: `capability_cone.rs:3866-3880` (base id set), `:3904-3906` (call split), `:3743`/`:4000`/`:4035` (`primary_app_guid` field + census); `registry.rs:449-456` (single-app map, every routine `"primary"`), `:524-543` (`cross_app_scope`); `gate/run.rs:244-249` (`is_primary_routine`), `:414-421` (`dependency_object_predicate`); `program/model/coverage.rs:243-250`; `l5/digest.rs:3700-3712`; `l4/effect_query_cli.rs:355-360`; `l5/selector_index.rs:80-95` (stable-id prefix); `l5/snapshot.rs:2359-2435` (gaps, stub `:2433`); `l5/snapshot_full.rs:381-411` (`project_apps`), `:1280-1286` (`serialize_sharded` role by `apps[0]`); `gate/diff/preflight.rs:25-30` (`apps[0]`); test helpers `l4/exec_relation.rs:353-359`, `l5/test_support.rs:635`. **New since the pre-draft:** 11b's `program/model/projection.rs:97-111` `role_of` decides a row's role from its `source_unit_id` (`ws:` = primary, `dep:<guid>:` = that app), a third rule | `p1-sites.txt`; `rg` at `f96619ec` |
| P1b prefix test vs rule (CDO) | **holds.** 31,658 rows: stable-id prefix role vs app_guid role disagree 0; object-id guid vs `app_guid` (verbatim) disagree 0; `dep_routine_ids` vs rule disagree 0. 11,133 objects: stable object-id prefix vs rule disagree 0. Same 0s on 4 dependency fixtures | `p5p6.err`, `p6-fx-*.err` |
| P1c (new) 11b `role_of` vs rule | **holds.** CDO: `ws:` rows with a dependency guid 0, `dep:` rows with the primary guid 0, `dep:<guid>` unit guid ≠ `app_guid` 0. Fixtures: same 0s; symbol-only rows have neither prefix (fall back to `app_guid`) | `p1c.err` |
| P2 `dep_routine_ids` reads | 14 files, **71** reads (69 at `5c42b8f3`; 11b added 2) | `rg -c` |
| P3 single-app names a dependency app? | **apps[] unaffected; the letter of the prediction falsified.** Two dependency fixtures: only the workspace guid outside `inputs`. CDO single-app fingerprint: 5 dependency guids appear outside `inputs`, all in `eventDeclarations[].eventId` / `.binding.publisherObject` (88 rows each: primary subscribers of dependency events). `identities` and every other key hold the primary app only; `apps` = the primary row; `analysisGaps` = [] | `p3-*.json`, `p3-cdo.json` (jq path census) |
| P4 cli-b census | **holds.** 62 files carry schema 3: 44 `snapshot/*.{raw,envelope}.json`, 12 `diff/snapshots/*.snap.json` (inputs), 2+2 `snapshot/shards/ws-d8-commit-in-tx/{all,primary}`, 2 `fingerprint/shards/ws-d8-commit-in-tx`. The `appGuid` list is the same 62. 86 CBOR files (44 `snapshot/`, 42 `fingerprint/`). Every `snapshot/*.raw.json` has `"analysisGaps": []`. No digest, prove, fingerprint-query or human file in either list | `p4-v3.txt`, `p4-apps.txt` |
| P4b stdlib CBOR decoder | **holds.** 86 of 86 CBOR files (`snapshot/` and `fingerprint/`, `.cbor` and `.cbor.gz`) decode equal to the matching `snapshot/*.raw.json` after dropping `undefined` | `cbordec.py`, `p4b.txt` |
| P5 gap loops on CDO, stub = "not primary" | **loop 3 dead: holds; and there is nothing to gap on CDO.** CDO cross-app: parse-incomplete 0 (primary and dependency), body-less dependency rows 0, `symbol-only-boundary` apps 0, `body-unavailable` 0, `FreshCoverage.opaque_apps` 0 (every CDO dependency has source). Fixtures: body-less rows exist only in symbol-only apps (`apps_mixed_body` = 0 everywhere): the model adds EVERY ABI row of a required symbol-only app (`workspace.rs:1600-1630`) and only parsed rows of an app with source | `p5p6.err`, `p6-fx-*.err` |
| P6 reach vs demand | **holds; CDO witnesses the reach filter on rows, not on gaps.** CDO: cone 57,532 edges, 26,179 dependency rows, 22,937 reachable from a primary routine (3,242 not), over the cut graph (primary callers + event dispatch) only 361: so F9 fires (the snapshot base cannot see reach). BFS 17 ms (release-fast, indicative). Fixture `tests/r2-5b-fixtures/cross-app-resolution`: 2 symbol-only apps, 8 body-less rows, 2 reachable; app `eeee…0002` owns no reachable row | `p5p6.err`, `p6-fx-tests_r2-5b-fixtures_cross-app-resolution.err` |
| P7 11b homogeneity on CDO | **holds (not re-run, as instructed).** `f96619ec` is 11b's merge; its numbers file records `cdo-gate` exit 0 on C and B heads (`docs/measurements/2026-10-22-issue-160-step11b.md:67-71`) and the merge gate re-ran it on the attested head. The assertion is `projection.rs:48` (`groups_mixed_role`) | `git log -1 f96619ec` |
| P8 role rule vs registry map (CDO) | **holds.** `cross_app_scope` map, today's vs rule-based: 42,625 keys each, disagree 0 (`analyze --scope primary`, and again inside `digest`) | `p8.err`, `p5p6.err` |
| P9 diff on a v99 copy | **holds.** Refused at load: `failed to load snapshot: deserializeSnapshot: unknown schemaVersion 99 (this build only handles v3)`, exit 1. Extra: a FATAL preflight (`app-identity-mismatch`, two vendored inputs of different apps) prints the diagnostic and exits **0**, also with `--fail-on low` | `p9.err`, `p9b.out` |
| P10 `tests/differential.rs:1758-1799` | **holds.** Lines 1758-1799 are the R2d coverage comment and `L3COV_FORBIDDEN_KEYS` (`:1781`); last touched `576dd296` (2026-10-19). Only fixture builders name `"contractFacts"`/`"permissionFacts"` in tests (`cli_b_diff_differential.rs:702-709,817-824`) | `sed`, `rg` |
| P11 inventory doc carries `apps` | **holds.** `ws-d35 --inventory-only`: `schemaVersion "1.1.0"`, `payload.apps` = the primary row | `p11.json` |
| P12 version consumers | **holds.** Envelope literal `snapshot_full.rs:1033`; `INVENTORY_SCHEMA_VERSION` `:1086`; `cli_p1_inventory.rs:101,476`; `snapshot_deserialize.rs:546` (unit-test JSON); doc comment `gate/run.rs:106`. Snapshot schema constants: `snapshot_full.rs:40` (pub), `snapshot_deserialize.rs:38` (private copy); hand-built v3 trees in `cli_b_diff_differential.rs:493,551` | `rg` |

Verdict: the design stands with four changes. (1) F9 fires: the snapshot's base is the cut, so
reach is computed from `rel.cone()` passed in by the caller (P6). (2) 8A-5 is withdrawn: body-less
dependency rows are exactly the symbol-only apps' ABI rows, so today's app grouping already is
"the opaque app" (P5); F6's branch is taken by evidence. (3) F7 fires (no step 6 ledger): the
Decision 8 test asserts the gap alone. (4) P1 adds 11b's `role_of` (kept, pinned by an
agreement test). The gap and key-class filters have no CDO witness (CDO has no symbol-only or
parse-incomplete dependency row); `tests/r2-5b-fixtures/cross-app-resolution` is the witness.

## Base/Head SHAs

- claim base: `f96619ecf27d777b6f3769661e4145ff0c5ee5d5` (origin/master at make-worktree)
- rebase 1 (after Task 2): onto `459294f1` (#223, epic step 5). Old head `b5f16a25`. Clean, no conflict, no charge. Range-diff: commits 1-8 `=`; Task 2 `!` only in `tests/cli/main.rs` and `tests/gap/gap_issue100_testfield_capability.rs` context lines (step 5 added `mod cli_step5_coverage_fold`). Step 5 moved no cli-b golden. Register re-pointed: O4N1 `fix_commit` 28aede25 → 9b02034b (spec text unchanged, every anchor_hash unchanged).

## Timeline

- 2026-10-22 22:1x claim (attempt 1), make-worktree ok, base f96619ec.
- Probes P1-P12 (+P1c) run by an opus author agent; P7 not re-run (11b's merge gate). Spec written, committed 5009a387.
- Spec panel (reviewers: gpt-6.1-sol in slot astra, Opus agent in slot flash; operator's stand-ins, `--substitute astra=sol --substitute flash=opus`):
  - r1 (spec_rounds 1/5): sol 17 findings (0c/10i/7m), opus 18 (0c/7i/11m). All 35 fixed, none refuted → a48b4a9f.
  - r2 (2/5): opus accepted 35/35; sol 33/35 (re-raised S3, O18); new SN1-SN4 (3i/1m), ON1-ON4 (4m). Fixed → 9f7cd309.
  - r3 (3/5): 20 ids; opus 20/20; sol 19/20 (re-raised SN2); new S3N1-S3N3 (3i), O3N1 (i), O3N2 (m). Fixed → 61040fb6.
  - r4 (4/5): 18 ids; both 18/18. New O4N1 (m) → 28aede25.
  - r5 (5/5): O4N1 accepted by both. **Converged**: 49 entries, all fixed, all accepted by both, every anchor_hash matches.
  - Review files: `.agent/runs/20261022-220347-220d76/spec-r{1..5}-{sol,opus}.md`.
- Acceptance tests (step 6): opus agent; 32 of 33 red for the stated reason, T2 green by construction (see `## Acceptance tests (red)`). Not committed.
- Plan (step 7): `docs/superpowers/plans/2026-10-22-issue-220-roles-snapshot-schema-4.md`, 81df9190; 8 tasks (0-7), plan_tasks charged 8 (4 left).
- Task 0 done by the conductor instead of the plan's `git stash -u` (the stash would have taken the uncommitted evidence files, and stash refs are shared with the main checkout and sibling worktrees): every red file copied to `/work/bench/160-8/red/` (MANIFEST, SHA256SUMS, `red-tracked.patch`), copies verified with `cmp`, then `git restore -- src tests` and the two new files removed. Each task copies its own red tests back in.
- Implementation (step 8): opus implementer + sonnet reviewer per task (Tasks 5+6 shared one reviewer); review findings went back to the same implementer. Commits: T1 31e81670 (+57f4c34e), T2 b5f16a25 → 2b1c9f9c after rebase (+d585c424), T3 acdfff29 (+61f22f44), T4 4b43d1a5 (+a06fe162), T5 1df5fd20, T6 81279914 (+79138815), T7 9cfbe13f. task_attempts: T1 1, T2 1, T3 1, T4 3 (cap reached, no further attempt needed), T5 1, T6 1, T7 1.
- Rebase 1 after Task 2 (see Base/Head SHAs). Supervised task gate on the rebased head: rc 0.
- Step 9: supervised `ci-steps-task` on 9cfbe13f rc 0 (27 s). Branch pushed; draft PR #229. GitHub CI on 9cfbe13f: test, release, GitGuardian all pass.
- Final panel (step 10): built-in `code-review` skill at high (ran as the Skill tool offered it; ids C1-C10) + sol + opus.
  - r1 (final_rounds 1/5): sol 7 (3i/4m, do not ship), opus 12 (1i/11m, do not ship), code-review 10. Fixed 25, refuted 3 (C3, C4, C9), deferred 1 (C6, minor, step 9) → 4a1fe398, 1054ce5b, 5995fae7, 86611392.
  - r2 (2/5): opus 29/29 accepted, ship; sol 26/29 (re-raised F6, G5, G7), no new. Fixed → 225fb0a6, b15eb333 (docs).
  - r3 (3/5): F6, G5, G7 accepted by both; no new findings; both **ship**. **Converged**: 78 entries, 0 open, all accepted by both, no blocking entry deferred.
  - Review files: `.agent/runs/20261022-220347-220d76/final-r{1,2,3}-{sol,opus}.md`, `final-r1-codereview.md`.

## Acceptance matrix

Spec §6, with the tests and proof rows (`## Discrimination proofs`) that carry each item.

| # | Evidence | Status |
|---|---|---|
| A8.1 | T1, T1c, T2 (proof rows T1 ×3, T1c ×2, T2); grep re-run at the final head, see `## Gate results` | met |
| A8.2 | T3, T4, T4c, T5; final panel C1 `parse_error_body_is_not_symbol_only` | met |
| A8.3 | T9a, T9b, T9d, T9e, T9f, T9g, T9-keys (composed + shard half), T9-closure ×4 (+ F1 identity arms, F6 every-leaf census), T10, T12; final panel G1 `gap_app_gets_its_apps_row`, G5 `single_app_universe_binds_a_single_app_model` | met |
| A8.4 | `SNAPSHOT_SCHEMA_VERSION: i64` once (Task 2 gate row) | met |
| A8.5 | T8 (+ O13, C8, G4 arms), T8b, T9f | met |
| A8.6 | T6, T6b, T7, T7b (+ F3/G6, F4 arms), T8b, C2 `preflight_identity_ignores_guid_case` | met |
| A8.7, A8.9 | `allowlist.py` (Part 0), see `## Gate results` | met (final run after the final panel) |
| A8.8 | T3, T6, T8, T8b, T9a-f | met |
| A8.10 | T11 `cli_p1_inventory` | met |
| A8.11 | `cli_workspace_fingerprint_closure` unchanged (Task 2 gate row) | met |
| A8.12 | not deliverable as written (spec §6); T9-keys stands in | as specified |
| A8.13 | no bench (Decision 58) | as specified |
| §8 Part 1-2, I2 CDO identity, E14 `cdo-gate` | not run yet: the conductor runs them after the final panel | pending |

## Acceptance tests (red)

Written 2026-10-22 on `28aede25` (uncommitted; they land with their implementation). Lib =
`cargo test -p al-sem --lib <name>`; cli = `cargo test --test cli cli_snapshot_schema4::<name>`.
"stub" = the test reaches a new API whose body is `unimplemented!("#220")`, after every
precondition it states held.

| test | file | fails with (one line of the real output) | reason matches spec? |
|---|---|---|---|
| T1 `role_rule_hand_stated` | `src/program/model/role.rs` | `panicked at src/program/model/role.rs:33:9: not implemented: #220` | yes (stub: `role_of_app`) |
| T1c `projection_role_of_agrees_with_rule` | `src/program/model/projection.rs` | `panicked at src/program/model/role.rs:39:9: not implemented: #220` | yes (stub `routine_role`; both preconditions held: symbol-only row and an upper-case `dep:` guid) |
| T2 `cross_app_scope_uses_role` | `src/engine/l5/registry.rs` | **passes** | n/a: pins behaviour today's `cross_app_scope` already has (lowercase compare = the rule, P8). Its break ("`role_of_app` always Primary") only exists once the registry calls the rule; it is the use test after conversion |
| T3 `gaps_reachable_only` | `tests/cli/cli_snapshot_schema4.rs` | `precondition: in-memory symbol-only-boundary for dddddddd-…0001: []` | yes (stub `is_dependency_role` = false; the eeee… reach preconditions held) |
| T4 `parse_incomplete_gap_filter` | same | `W is gapped with role primary … left: Some(Null) right: Some(String("primary"))` | yes (gap `role` missing; R-not-in-reach / A-in-reach / R-is-model-row preconditions held, F10 not needed for the shape, flags hand-stated) |
| T4c `gap_reach_follows_dependency_calls` | same | `Inner's parse-incomplete gap is serialized: [{… "subject": "bbbb7070-…:Codeunit:10#12c2…"}]` (no `role`) | yes (cone-yes / cut-no preconditions held; fails on the `role` half; the cut break bites only once a reach filter exists) |
| T5 `symbol_only_callee_has_its_gap` | same | `dddd…'s symbol-only-boundary gap: []` | yes (stub false; DriveDeps→body-less dddd edge precondition held) |
| T6 `diff_refuses_mixed_universes` | same | `precondition: …/old.json loads: deserializeSnapshot: unknown schemaVersion 4 (this build only handles v3)` | partly: fails at "both load", because v4 is not loadable yet; the mismatch assertion is reached only after D5 |
| T6b `fatal_preflight_exits_one` | same | `a fatal preflight exits 1  left: 0 right: 1` | yes (P9) |
| T7 `diff_refuses_schema_3` | same | `failed to load snapshot: deserializeSnapshot: unknown schemaVersion 4 (this build only handles v3)` (on `msg.contains("schemaVersion 3")`) | yes |
| T7b `diff_refuses_bad_universe_or_roles` | same | `no-universe: the message names "analysisUniverse": failed to load snapshot: … unknown schemaVersion 4 …` | yes (one-field-differs precondition held for all five) |
| T8 `shards_read_role_not_position` | `src/engine/l5/snapshot_full.rs` | `left: Some("dependency") right: Some("primary")` (manifest: `00000000-…d0` got `primary.json`) | yes (break 1 shape) |
| T8b `preflight_identity_reads_role` | `src/engine/gate/diff/preflight.rs` | `panicked at src/engine/gate/diff/preflight.rs:152:9: []` (no diagnostic) | yes |
| T9a `key_class_inventory` | cli | `contractFacts: no dependency row  left: [{… "stableId": "bbbb7070-…:Codeunit:10" …}] right: []` | yes |
| T9b `key_class_behaviour` | cli | `capabilityFacts: no dependency subject: [{… "subject": "bbbb7070-…:Codeunit:10#12c2…" …}]` | yes |
| T9d `key_class_evidence` | cli | `no Inner operation row` | yes (break 1 shape) — see deviation 1 |
| T9g `evidence_follows_dependency_witness` | `snapshot_full.rs` | `panicked at src/engine/l5/snapshot_full.rs:162:5: not implemented: #220` | yes (stub `filter_by_class`; precondition held) |
| T9-closure fields `closure_fields_are_classified_and_closed` | cli | `DD: id-shaped values under fields in neither REFERENCES nor NON_REFERENCE_FIELDS: {"apps.appGuid", "callsiteIndex.callsiteId", …}` | yes (empty stub lists) |
| T9-closure domains `closure_reports_each_dangling_domain` | cli | `panicked at src/engine/l5/snapshot_full.rs:148:5: not implemented: #220` | yes (stub `closure_violations`) |
| T9-closure nested (F30) `closure_nested_references_keep_their_rows` | `snapshot_full.rs` | `not implemented: #220` (filter_by_class) | yes (each "referenced once" precondition held) |
| T9-closure literal (F30) `closure_literal_keeps_nothing` | `snapshot_full.rs` | `not implemented: #220` (filter_by_class) | yes |
| T9e `key_class_input` | cli | `precondition: the cross-app filter ran  left: Null right: "cross-app"` | yes — see deviation 2 |
| T9f `key_class_header_and_apps_order` | cli | `left: Null right: "single-app"` (`analysisUniverse`) | yes |
| T9-keys composed `composed_keys_are_classed` | cli | `single_app=true: schemaVersion has no class ([])` | yes |
| T9-keys shard `slice_for_app_handles_every_classed_key` | `snapshot_full.rs` | `SNAPSHOT_KEYS lists every top-level key` | yes (empty stub) |
| T10 `no_dependency_cross_equals_single` | cli | `left: Null right: "single-app"` | yes (gaps / Unused preconditions held) |
| T11 `cli_p1_inventory` (2 tests edited) | `tests/cli/cli_p1_inventory.rs` | `inventory doc schemaVersion must be '1.2.0'  left: "1.1.0"` | yes |
| T12 `cross_app_universe_binds_its_model` | cli | `composing A's model with B's universe must panic: Map({"schemaVersion": Int(3), …` | yes (equal-ids precondition held) |
| `wrong_schema_version_errors` (rewritten, D5.1) | `src/engine/gate/snapshot_deserialize.rs` | `schemaVersion 3 is refused: Map({"schemaVersion": Int(3)})` | yes |

Deviations from the spec's letter:
1. **T9d: "in memory, `Outer`'s callsiteIndex row exists" is false at this base.**
   `derive_callsite_evidence` (`snapshot.rs:1974`) keeps only call sites a cut fact or cut typed
   edge references; `Outer -> Inner` is cone-only. The test asserts the opposite (with a
   message to restore the spec's precondition once step 6 makes it true), so the "NOT
   `Outer`'s call-site row" arm is vacuous until step 6. `Inner`'s operation row does exist.
2. **T9e** gains the precondition `analysisUniverse == "cross-app"`; without it both trees are
   unfiltered today and the test passes vacuously.
3. **T10's "no REFERENCES field names Unused"** is checked through `REFERENCES` (defining
   fields excluded); `schemaFacts`/`contractFacts` `stableId` (defining) do name it. Vacuous
   while `REFERENCES` is the empty stub.
4. **T9-closure** is split in four: two composed (DD+R5B classification + Closure; DD per-domain
   dangling arms, injected into the composed tree), two on hand-built typed rows (nested
   value sources + `witnessFirstHopOperationId` through `permissionFacts`; the literal arm),
   as F30 allows. The path-matching rule the classification uses is documented on `REFERENCES`.
5. **T2 passes before the implementation** (row above).
7. **`NON_REFERENCE_FIELDS` is per key** (`&[(&str, &[&str])]`, `key_classes.rs`), not the
   spec's flat `&[&str]`: one bare data name (`kind`, `value`, `role`) is data in one key and
   must not mask a same-named field of another key (final panel G10; recorded in the Task 4
   proof notes until then).
8. **`EXTERNAL_IDENTITY_SHAPES` is a predicate inside `closure_violations`**, not a named
   constant: the shape needs the model's app set and routine map, which a `const` cannot hold.
   Since final panel F1 it is exactly the spec's two shapes ("unknown"; an id rooted in a bare
   guid the model does not hold and equal to no model routine id) and fails closed otherwise.
6. Not rewritten here (spec §5 "rewritten with the code they pin"; green today, they move with
   the constant): `snapshot_deserialize` `roundtrip_*` (`Int(3)`, decode only), and
   `cli_b_diff_differential.rs` `snapshot()`, the schema-mismatch oracle `:551`, the
   app-identity oracle's app `role`s.

Stubs added (no real implementation):
- `src/program/model/role.rs` (new, `pub mod role` in `model/mod.rs`): `Role {Primary,
  Dependency}`, `Role::as_str`, `Model::role_of_app` / `routine_role` / `object_role`,
  `role_of_stable_id` — bodies `unimplemented!("#220")`.
- `snapshot_full.rs`: `FullSnapshotOptions.universe`; `SnapshotUniverse {SingleApp,
  CrossApp(CrossAppUniverse)}`; `CrossAppUniverse { base }` (private); `SnapshotUniverse::of`
  (a real two-arm match: a constructor, so T12/T9 reach their assertions);
  `compose_full_snapshot` IGNORES `universe` (today's behaviour). `KeyClass`, `Domain`,
  `SNAPSHOT_KEYS = &[]`, `REFERENCES = &[]`, `NON_REFERENCE_FIELDS = &[]`, `ClosureViolation`,
  `pub fn closure_violations(tree, resolved, universe)` and
  `pub(crate) fn filter_by_class(core, permission_facts, primary_app_guid, reachable)` —
  bodies `unimplemented!("#220")`. Signatures are the implementer's to change, with the tests.
- `snapshot.rs`: `#[cfg(test)] RoutineOrderFrames::from_entries` (hand-built frame tables).
- Compile-only `universe: SnapshotUniverse::SingleApp` at both production callers
  (`fingerprint_cli.rs`, `gate/diff/cli.rs`) and the four O11 tests.

## Discrimination proofs

| test | mutation | fail output | pass output | commit |
|---|---|---|---|---|
| T1 `role_rule_hand_stated` | `role.rs:37` case-sensitive `p.app_guid != app_guid` | `role.rs:140` `role_of_app(&upper)`: left Dependency, right Primary | ok (1 passed) | 31e81670 |
| T1 `role_rule_hand_stated` | `role.rs:36-39` None arm dropped (`Some(p) if eq => Primary, _ => Dependency`) | `role.rs:150` `none.role_of_app(DEP)`: left Dependency, right Primary | ok (1 passed) | 31e81670 |
| T1 `role_rule_hand_stated` | `role.rs:64` `:` check removed in `role_of_stable_id` | `role.rs:165` "a primary guid not followed by `:` is no prefix": left Primary, right Dependency | ok (1 passed) | 31e81670 |
| T1c `projection_role_of_agrees_with_rule` | `projection.rs:103` `role_of` returns None for `dep:` units too | `projection.rs:512` Work (CCCC0220…, unit dep:…): left None, right Some("cccc0220-…") | ok (1 passed) | 31e81670 |
| T1c `projection_role_of_agrees_with_rule` | `projection.rs:110` `role_of` drops `to_ascii_lowercase` | `projection.rs:512` left Some("CCCC0220-…"), right Some("cccc0220-…") | ok (1 passed) | 31e81670 |
| T2 `cross_app_scope_uses_role` | `role.rs:37` `role_of_app` always `Primary` (registry now calls the rule) | `registry.rs:965` "--scope primary": left ["dep-located","on-dep-object","on-dep-routine"], right ["dep-located"] | ok (1 passed) | 31e81670 |
| u-1 `single_app_scope_uses_role` | `registry.rs:454` single-app map value constant `"primary"` | `registry.rs:1333` "a dependency row's finding": left 1, right 0 | ok (1 passed) | 57f4c34e |
| u-1 `single_app_is_primary_routine_uses_role` | `gate/run.rs:247` `SingleApp(_) => true` arm restored | `run.rs:1226` assertion failed: `!target.is_primary_routine(&r)` | ok (1 passed) | 57f4c34e |
| u-3 `dependency_object_predicate_unknown_object_is_false` | `gate/run.rs:417` `is_some_and` → `is_none_or` (unknown → true) | `run.rs:1232` assertion failed: `!dependency_object_predicate(&m, &idx)("dep/Codeunit/1")` | ok (1 passed) | 57f4c34e |
| T7 `diff_refuses_schema_3` | `snapshot_deserialize.rs:82` loader accepts 3 (`Some(SNAPSHOT_SCHEMA_VERSION \| 3)`) | `cli_snapshot_schema4.rs:130` `outcome.exit_code`: left 0, right 1 (v3 loaded, diff ran) | ok (cli_snapshot_schema4 2 passed) | b5f16a25 |
| `wrong_schema_version_errors` | same break as T7 | `snapshot_deserialize.rs:567` "schemaVersion 3 is refused: Map({"schemaVersion": Int(3)})" | ok (1 passed) | b5f16a25 |
| T12 `cross_app_universe_binds_its_model` | `snapshot_full.rs:122-125` the `ptr::eq` assert deleted | `cli_snapshot_schema4.rs:169` "composing A's model with B's universe must panic: Map({"schemaVersion": Int(4), …" | ok | b5f16a25 |
| T11 `cli_p1_inventory` (both tests) | `snapshot_full.rs:1168` `INVENTORY_SCHEMA_VERSION` back to `"1.1.0"` | `cli_p1_inventory.rs:100` left "1.1.0" right "1.2.0"; `:485` left Some("1.1.0") right Some("1.2.0") | ok (6 passed) | b5f16a25 |
| T12 (review F3, full-message assert) | `snapshot_full.rs:124` panic message changed to "...the model is not the universe's model" (the old `contains("model")` would have passed) | `cli_snapshot_schema4.rs:175` left "compose_full_snapshot: the model is not the universe's model", right "...not the cross-app universe's model" | ok (1 passed); task-affected rc 0 (3680 passed); check-diff `[]` | d585c424 |
| T11 `inventory_only_projection_subset_self_consistency` | `build_inventory_doc` drops `payload.insert("analysisUniverse", …)` | `cli_p1_inventory.rs:109` "the inventory payload carries analysisUniverse" left Null right "single-app" | ok (6 passed) | b5f16a25 |
| T3 `gaps_reachable_only` | `snapshot_full.rs:496` reach filter skipped (`true \|\| g.role == …`) | `cli_snapshot_schema4.rs:264` "the unreachable opaque app has no serialized gap: [… symbol-only-boundary … eeee…]" | ok (6 passed) | acdfff29 |
| T3 `gaps_reachable_only` | `snapshot.rs:2366` `is_dependency_role` back to `false` | `cli_snapshot_schema4.rs:252` "precondition: in-memory symbol-only-boundary for dddddddd-…0001: []" | ok (6 passed) | acdfff29 |
| T4 `parse_incomplete_gap_filter` | same break as T3's first (no reach filter) | `cli_snapshot_schema4.rs:381` "R (unreachable) has no serialized gap: [… R's parse-incomplete …]" | ok (6 passed) | acdfff29 |
| T4 `parse_incomplete_gap_filter` | `snapshot.rs:2379` parse-incomplete `role` hard-coded `"dependency"` | `cli_snapshot_schema4.rs` "W is gapped with role primary": left Some("dependency") right Some("primary") | ok (6 passed) | acdfff29 |
| T4c `gap_reach_follows_dependency_calls` | `snapshot_full.rs:447` reach BFS over `rel.primary().edges_by_from` (the cut) instead of `rel.cone()` | `cli_snapshot_schema4.rs` "Inner's parse-incomplete gap is serialized: []" | ok (6 passed) | acdfff29 |
| T5 `symbol_only_callee_has_its_gap` | same break as T3's second (`is_dependency_role` = `false`) | `cli_snapshot_schema4.rs` "dddd…'s symbol-only-boundary gap: []" | ok (6 passed); task-affected rc 0 (3684 passed); check-goldens rc 0, no golden moved; check-diff `[]` before and after | acdfff29 |
| review F-3a `gap_app_subject_matches_case_insensitively` | `snapshot_full.rs:479` `app_key` = `guid.to_string()` (case fold dropped) | `cli_snapshot_schema4.rs` "the lower-case dddd… gap is kept: [..]" (gap dropped) | ok (8 passed) | 61f22f44 |
| review F-3b `opaque_gap_of_an_app_with_source_fires` | `snapshot_full.rs:500` F21 `debug_assert!` condition made `true \|\| …` (assert removed) | "test did not panic as expected at cli_snapshot_schema4.rs:514" | ok (8 passed); task-affected rc 0 (3686 passed); check-goldens rc 0, no golden moved; check-diff `[]` before and after | 61f22f44 |
| T9a `key_class_inventory` (break A, identity filter) | `snapshot_full.rs` compose: `primary_app_guid(resolved).filter(\|_\| false)` (filter_by_class never runs) | `cli_snapshot_schema4.rs:691` "rootClassifications: no dependency row" left [bbbb…:Codeunit:10#12c2… row] | ok (lib 9, cli 18 passed) | 4b43d1a5 |
| T9b `key_class_behaviour` | same break A | `:757` "every primary fact stays" left 15 (dependency facts serialized) | ok | 4b43d1a5 |
| T9d `key_class_evidence` break 1 (emit every row) | same break A | `:874` "no Inner operation row" | ok | 4b43d1a5 |
| T9a `key_class_inventory` (break B, derive identity filter) | `snapshot_full.rs` `derive_contracts` objects and `derive_schema` tables filter `true \|\| role == Primary` | `:691` "contractFacts: no dependency row" left [bbbb…:Codeunit:10 object row …] | ok | 4b43d1a5 |
| T9a `key_class_inventory` (break C, routineInventory) | `build_inventory_doc` routine filter `true \|\| …` | `:705` "routineInventory lists no Outer" | ok | 4b43d1a5 |
| T9f `key_class_header_and_apps_order` (sort) | `project_apps`: whole `apps[]` re-sorted by `appGuid` after the extend | `:1322` left "dddddddd-…0001" right "ffffffff-…00aa" | ok | 4b43d1a5 |
| T9f (owner filter, review F4) | `project_apps` deps filter `true \|\| owners… \|\| referenced…` | `:1330` "no eeee… row: [ffff…, dddd…, eeee… …]" | ok | 4b43d1a5 |
| T9f (case-insensitive match, review F4; new upper-case arm, hand-stated `DependencyApp.guid`) | deps filter `let g = a.guid.clone()` (case fold dropped), F22 assert also removed in the same run | `:1301` "the upper-case dependency app owns its identities: [aaaa… primary only]" | ok | 4b43d1a5 |
| `identity_app_missing_from_dependency_apps_fires` (F22, review F4) | F22 `debug_assert!` in `project_apps` deleted (same run as above) | "test did not panic as expected at cli_snapshot_schema4.rs:538" | ok | 4b43d1a5 |
| `composed_keys_are_classed` | compose inserts `zzUnclassed: Null` after `eventDeclarations` | `:1356` "single_app=true: zzUnclassed has no class ([…])" | ok | 4b43d1a5 |
| T10 `no_dependency_cross_equals_single` break 1 (role only cross-app) | compose clears every gap `role` in `SingleApp` | `:1432` "the single-app gap rows carry role primary: […"role": "" …]" | ok | 4b43d1a5 |
| T10 break 2 (Evidence keeps referenced ids only) | `key_classes.rs` step 5 `(false && is_primary(id)) \|\| refs…` | `:1443` "the trees differ only in analysisUniverse" (Unused gone cross-app) | ok | 4b43d1a5 |
| T9d break 2 (primary only) | step 5 `is_primary(id) \|\| (false && refs…)` | `:854` "Outer (referenced) is an identity" | ok | 4b43d1a5 |
| T9d break 4 (drop primary Evidence rows) | step 3 operations `(false && is_primary(..)) \|\| referenced` | **came back GREEN on T9d**: TouchCust's op row is referenced by its own fact's `witnessOperationId`; every primary Evidence row of a composed tree is referenced by its routine's fact or edge. Moved to hand-built rows (F30 pattern): new lib test `evidence_keeps_unreferenced_primary_rows` failed `snapshot_full.rs:2570` "the primary routine's unreferenced operation row is kept" | ok | 4b43d1a5 |
| `evidence_keeps_unreferenced_primary_rows` (call-site arm) | step 3 call sites `(false && is_primary(..)) \|\| referenced` | `snapshot_full.rs:2574` "the primary routine's unreferenced call-site row is kept" | ok | 4b43d1a5 |
| T9g `evidence_follows_dependency_witness` break 1 | `Referenced::add` Operation → `operations.extend(identity_key(..))` (identity mapping) | `snapshot_full.rs:2527` "the witnessed dependency operation row is kept" | ok | 4b43d1a5 |
| T9g break 2 (skip step 4) | step 4 `refs.add` of both indexes skipped and the frame join `(false && evidence_routines…)` | `snapshot_full.rs:2535` "the kept Evidence rows' frame table is kept"; same run DD `closure_fields` reports `operationIndex.routine` bbbb…:Codeunit:11#… unresolved (the identity half) | ok | 4b43d1a5 |
| T9-closure break 1 (read every Text) `closure_literal_keeps_nothing` | `row_references`: an unmatched `Text` leaf → `Identity` | `snapshot_full.rs:2738` "a literal value is data: it keeps nothing" | ok | 4b43d1a5 |
| T9-closure break 2 (drop `tableId`) `closure_fields_are_classified_and_closed` | `TABLE_ID` renamed `tableIdDropped` | first run **GREEN on the composed test** (no table-field source in DD/R5B: F28 fired); after hand-stating one on TouchCust's kept fact: `:1124` "DD: id-shaped values under fields in neither …: {capabilityFacts.resourceArgSource.tableId}"; lib nested/literal also fail | ok | 4b43d1a5 |
| T9-closure break 3 (`/`→`:` table rewrite) | `identity_key` table arm → `parts[..3].join("/").replace('/', ":")` | `:1104` "DD: [… capabilityFacts.resourceId … bbbb7070-…/table/18 …]" | ok | 4b43d1a5 |
| T9-closure break 4 (`stableId` as reference) | `schemaFacts` refs `[("stableId", Identity)]` | T10 `:1425` "precondition: no reference field names Unused: [schemaFacts.stableId = …:Table:50199, …#1]" | ok | 4b43d1a5 |
| T9-closure break 5 (only `resourceArgSource`) `closure_nested_references_keep_their_rows` | capabilityFacts `TABLE_ID` → `("resourceArgSource.tableId", Identity)` | `snapshot_full.rs:2649` "extra.bodyArgSource: the referenced dependency table identity is kept" | ok | 4b43d1a5 |
| T9-closure break 6 (no `initializer` recursion) | capabilityFacts `TABLE_ID` → `resourceArgSource.tableId` + `extra.*.tableId` | `snapshot_full.rs:2681` "constant-var initializer, two levels deep: …" | ok | 4b43d1a5 |
| T9-closure break 7 (drop permission `witnessFirstHopOperationId`) | entry removed from `permissionFacts` refs | `snapshot_full.rs:2665` "derivedFromCapability.witnessFirstHopOperationId: the operation row is kept" | ok | 4b43d1a5 |
| T9-closure break 8 operation `closure_reports_each_dangling_domain` | `Operation => true \|\| …` | `:1220` "Operation: the dangling mi/nohash/op99 is reported: []" | ok | 4b43d1a5 |
| break 8 callsite | `Callsite => true \|\| …` | `:1246` "Callsite: the dangling mi/nohash/cs99 is reported: []" | ok | 4b43d1a5 |
| break 8 edge | `Edge => true \|\| …` | `:1246` "Edge: the dangling nosuchedge00000 is reported: []" | ok | 4b43d1a5 |
| break 8 frame | `Frame => true \|\| …` | `:1246` "Frame: the dangling 999 is reported: []" | ok | 4b43d1a5 |
| break 8 app | `App => true \|\| …` | `:1246` "App: the dangling 99999999-…0099 is reported: []" | ok | 4b43d1a5 |
| `parentFrameId = -1` arm | `-1` exception `false && …` | `:1236` "parentFrameId = -1 is no reference: [routineOrderFrames.parentFrameId -1 …]" | ok | 4b43d1a5 |
| F25 shape (closure_fields R5B) | unanchored-site exception `false && …` | `:1130` "R5B: [callsiteResolutions.callsiteId …/cs1, …]" | ok | 4b43d1a5 |
| T9e `key_class_input` | compose drops `dep-package` input rows in CrossApp | `:1251` assertion `left == right` (inputs: single has the dep-package row) | ok; T9e was already green before the implementation (a guard; `analysisUniverse` landed in Task 2) | 4b43d1a5 |
| review 1: T9b `key_class_behaviour` (filter driven by `SNAPSHOT_KEYS`) | `key_classes.rs` `("coverage", KeyClass::Input)` (reclassed, so unfiltered) | `:780` "coverage: no dependency subject: [bbbb…:Codeunit:10#12c2… row]" | ok (lib 10, cli 19 passed) | a06fe162 |
| review 3: `closure_anchored_unresolved_site_is_a_violation` (new) | F25 condition `(true \|\| !anchored.contains(..))` | `:1253` "the anchored site …/cs0 is reported: [only typedEdges.callsiteId]" (resolution row passed as external) | ok | a06fe162 |
| review 2/4: `closure_fields_are_classified_and_closed` (calls production `is_classified`, per-key data lists) | `TABLE_ID` → `tableIdDropped` | `:1080` "DD: … neither …: {capabilityFacts.resourceArgSource.tableId}" | ok; per-key `NON_REFERENCE_FIELDS` census held on DD + R5B | a06fe162 |
| review 2: T10 (calls production `is_reference`) | `schemaFacts` refs `[("stableId", Identity)]` | `:1459` "precondition: no reference field names Unused: [schemaFacts.stableId = …:Table:50199, …]" | ok; task-affected rc 0 (3701 passed); check-goldens rc 0 (945), no golden moved; check-diff `[]` before and after | a06fe162 |
| T8 `shards_read_role_not_position` break 1 | `snapshot_full.rs:1510` role arm → `_ if guid == app_guid(&apps[0])` | `:2895` left Some("dependency") right Some("primary") | ok | 1df5fd20 |
| T8 break 2 (identities only) | `key_classes.rs:545-553` operation/callsite/edge `close_rows` calls removed | `:2912` "the witnessed dependency operation row is in the primary shard" | ok | 1df5fd20 |
| T8 break 3 (one pass) | `key_classes.rs:554` `if !grew` → `if grew \|\| !grew` | `:2924` "the operation's routine identity (second pass): [… no `…d0:Codeunit:10#op`]" | ok | 1df5fd20 |
| T8 break 4 (close frames) | `key_classes.rs:554` insert `full["routineOrderFrames"]` into the shard | `:2928` "a shard never carries routineOrderFrames" | ok | 1df5fd20 |
| `slice_for_app_handles_every_classed_key` | `key_classes.rs` `SNAPSHOT_KEYS` + `("probeKey", Inventory)` | `:2994` "probeKey is narrowed by the slice: [… ffff0220… ]" | ok; was already green before the implementation (Task 4's keys and the slice already agreed) | 1df5fd20 |
| T6 `diff_refuses_mixed_universes` | `preflight.rs:66` `if false && old_u != new_u` (universe check dropped) | `cli_snapshot_schema4.rs:648` kinds `[]` (no `analysis-universe-mismatch`; the diff ran) | ok | 81279914 |
| T6b `fatal_preflight_exits_one` | `diff/cli.rs:302` `(false && result.fatal) \|\| …` (fatal gate dropped; run on the final `\|\|` shape) | `:681` "a fatal preflight exits 1" left 0 right 1 | ok (also asserts exit 1 under `--fail-on low`, stdout equal) | 81279914 |
| T7b `diff_refuses_bad_universe_or_roles` check 1 (missing universe) | `snapshot_deserialize.rs:115` arm `None => {}` | `:778` "no-universe: the message names \"analysisUniverse\": " (loaded; exit 1 came from the universe preflight) | ok | 81279914 |
| T7b check 2 (unknown universe) | `:114` `Some(CborValue::Text(_)) => {}` | `:778` "both: the message names \"analysisUniverse\": " (loaded) | ok | 81279914 |
| T7b check 3 (row without role) | `:135` role arm `None => {}` | first run GREEN: the role-less only row also breaks exactly-one-primary, whose message contains "role". Test fixed: needle → `apps[0].role`. Re-run: `:780` "no-role: the message names \"apps[0].role\": … exactly one role \"primary\" row, found 0" | ok | 81279914 |
| T7b check 4 (two primaries) | `:143` `primaries != 1` → `primaries == 0` | `:778` "two-primaries: refused" left 0 right 1 | ok | 81279914 |
| T7b check 5 (dependency shard) | `:143` `primaries != 1` → `primaries > 1` | `:778` "dependency-shard: refused" left 0 right 1 | ok | 81279914 |
| T7b empty-`apps[]` arm (Review Focus) | `:143` `!apps.is_empty() &&` dropped | `:793` "an empty apps[] v4 file loads: … found 0" | ok | 81279914 |
| T8b `preflight_identity_reads_role` | `preflight.rs:32` `.find(\|a\| true \|\| …)` (= `apps[0]`) | `preflight.rs:175` diagnostics `[]` | ok | 81279914 |
| review R1: T8 upper-case arm (O13) | `snapshot_full.rs:1553` slice prefix → plain `s.starts_with(&format!("{app_guid}:"))` | `:2950` "an upper-case manifest guid keeps the primary's lower-case rows" | ok | 79138815 |
| review R1: T8 upper-case arm, apps filter | `:1580` `g.eq_ignore_ascii_case(app_guid)` → `g == app_guid` | came back GREEN: `serialize_sharded` passes each row's own `appGuid`, so the compare is always exact (a property of the code, not a test defect). Fix: kept the plain `==` (the pre-#220 code) with a comment saying why | ok | 79138815 |
| final F1: `closure_reports_each_dangling_domain` identity arms (`""`, `garbage`, `e09e…/nosuchroutine`) + external-shape pass arm | `key_classes.rs` external predicate `v.len() > root.len() && is_bare_guid(root)` → `true` (the old fail-open rule) | `:1403` "Identity: the dangling  is reported: []" | ok | 4a1fe398 |
| final C8: T8 anchor arm (`callAnchor.enclosingRoutineId = mi/anc`) | `snapshot_full.rs` `close_shard(.., &HashMap::new())` (no model map) | `:2962` "the anchor's routine identity, through the model: [… no #anc]" | ok | 4a1fe398 |
| final G4: T8 full-order assert | `close_rows` appends added rows after the kept ones instead of merging | `:2973` "the full tree's identity order" left […#to, …#anc, …] right […#op, …#to, …] | ok | 4a1fe398 |
| final G3/C7: T8 break 3 re-run on the incremental fixpoint | `key_classes.rs:585` `if !grew` → `if grew \|\| !grew` | "the operation's routine identity (second pass)" | ok | 4a1fe398 |
| final C2: `preflight_identity_ignores_guid_case` (new) | `preflight.rs` `!o.eq_ignore_ascii_case(n)` → `o != n` | `preflight.rs:196` `["app-identity-mismatch"]` | ok | 1054ce5b |
| final F4: T7b `unknown-role` arm (second row `role: "owner"` beside the primary) | `snapshot_deserialize.rs` dependency arm → `Some(CborValue::Text(_)) => {}` | `:813` "unknown-role: refused" left 0 right 1 | ok | 1054ce5b |
| final F3/G6: T7b `no-apps-key` and `apps-not-array` arms | `snapshot_deserialize.rs` apps match + `_ if true => &[]` (old accept) | `:813` "no-apps-key: refused" left 0 right 1 | ok; `normal_gz_snapshot_round_trips_under_cap`'s hand tree gained `apps: []` (F20: upgraded, rule not relaxed) | 1054ce5b |
| final F5: `v4_copy` asserts the vendored v4 header instead of re-applying it | `tests/cli-b-goldens/diff/snapshots/event-added-old.snap.json:177` `4` → `3` (stale input, temporarily) | `:563` "precondition: event-added-old.snap.json is v4" left 3 right 4 (the old helper would have upgraded it silently) | ok; input restored, `git status tests/` clean | 1054ce5b |
| final G12: T9d R5B precondition is the in-memory identities alone | `key_classes.rs:805` identities step → `true \|\| …` (keep all) | `:1146` "no eeee… identity: [eeeeeeee-…:Codeunit:60100, …]" | ok | 1054ce5b |
| final C1: `parse_error_body_is_not_symbol_only` (new; DD `Inner` hand-stated `parse_incomplete` + no body) | red before the fix = the break: `snapshot.rs` opaque loop without `!r.parse_incomplete` | `snapshot_full.rs:519` panic "a symbol-only-boundary gap names an app with source" (the F21 assert; C1 confirmed) | ok | 5995fae7 |
| final G1: `gap_app_gets_its_apps_row` (new; multi-hop: `Inner` → symbol-only `ZDeep.Work`) | `snapshot_full.rs:609` `\|\| referenced_apps.contains(&g)` → `\|\| (false && …)` | `:492` "ZDeep's apps[] row: [aaaa…, bbbb…]" | ok (first fixture used `Deep.Run()`, a codeunit built-in, so no edge: renamed `Work`) | 5995fae7 |
| final G2: T9a, `derive_contracts` routine filter | `snapshot_full.rs:671` `.filter(\|r\| true \|\| …)` | `:1036` "contractFacts: no dependency row" | ok | 5995fae7 |
| final G2: T9a, `derive_permissions` routine filter | `snapshot_full.rs:897` `.filter(\|r\| true \|\| …)` | `:1036` "permissionFacts: no dependency row" | ok | 5995fae7 |
| final G5: `single_app_universe_binds_a_single_app_model` (new) | `snapshot_full.rs:149` assert `true \|\| …` | `:1837` "a cross-app model composed as SingleApp must panic: Map({…})" | ok | 5995fae7 |
| final F6: `closure_fields_are_classified_and_closed` every-leaf arm | `key_classes.rs` `"externallyReachable"` → `"externallyReachableDropped"` | `:1444` "DD: fields in neither …: {rootClassifications.externallyReachable}" | ok; census first listed 27 unclassified numeric/boolean fields (DD) + `resultConsumed` (R5B), all classified from their serde structs (F17) | 5995fae7 |

## Findings register summary

`findings.json`: spec panel rounds 1-5, 49 entries (all closed before the plan); final panel
round 1 (register round 6), 29 entries: sol F1-F7, opus G1-G12, code-review C1-C10. Dispositions
are in the register; fixes in 4a1fe398, 1054ce5b, 5995fae7, 86611392 and `allowlist.py`.
Rounds 2-3 closed F6, G5, G7 (225fb0a6, b15eb333). Final state: 78 entries; dispositions fixed
74, refuted 3 (C3, C4, C9), deferred 1 (C6, minor); every entry accepted by both reviewers
(sol in slot astra, opus in slot flash; attest with `--substitute astra=sol --substitute flash=opus`).

## Gate results

- Task 1 (31e81670): `ci-steps task-affected` rc 0 (3667 passed, 8 skipped); `check-goldens` rc 0 (921 passed), no golden moved. CDO identity (§13.1): `alsem analyze <WS> --format sarif --scope primary`, base `alsem-base` (f96619ec) vs task tree (release-fast): byte-identical, both SHA-256 `0d0fad827861042c1be99a115521513a59db0d1ff5460c2ca23ba097f355099b` (9,698,287 bytes).
- A8.1 grep, expected test-only hits beyond the spec's list: `registry.rs:905,912,919` (T2's precondition lookups, kept independent of the rule on purpose so a broken rule cannot bend them) and `projection.rs:510` (T1c's expected value).
- Task 1 review fixes (57f4c34e): `ci-steps task-affected` rc 0 (3670 passed, 8 skipped).
- Task 2 (b5f16a25): before the regen `check-goldens` failed exactly the 8 cli-b snapshot/fingerprint golden tests (915/923). `check-goldens --regen` rc 0; then `check-goldens` rc 0 (923 passed, 4 skipped); `cli_workspace_fingerprint_closure` unchanged, 6 passed (A8.11); `ci-steps task-affected` rc 0 (3672 passed, 8 skipped); A8.4 `rg 'SNAPSHOT_SCHEMA_VERSION: i64' src` = 1. Golden move, mechanically: `python3 -I /work/bench/160-8/t2_golden_check.py /work/al-sem-issue-220-a1 57f4c34e` → `RESULT: OK`, 149 files (diff/snapshots 12, diff/PROVENANCE.md 1, snapshot 88, snapshot/shards 4, fingerprint 42, fingerprint/shards 2): after deleting only the version 3→4, `analysisUniverse: "single-app"` (right after `generatedAt` in CBOR, sorted place in JSON) and `apps[].role: "primary"` (last in CBOR rows), base == head with key order; no new golden file; nothing outside cli-b `snapshot/`, `fingerprint/`, `diff/snapshots/`, `diff/PROVENANCE.md`. Every diff OUTPUT byte-identical. `check-diff --base master` reports `protected-path:.agent/issue-160/{findings.json,ledger.md}` both before and after the commit: master moved to 459294f1 (#223) after the branch point; `--base f96619ec` (the merge base) reports `[]`.
- Task 4 (4b43d1a5): red recorded first (lib: 3 `not implemented: #220`; cli: 7 failed, T9e and T10 already green). `ci-steps task-affected` rc 0 (3700 passed, 8 skipped; the run before fixed two clippy-dev findings: `type_complexity` on `REFERENCES` → `KeyReferences`/`ReferenceField` aliases, and two test lints); `check-goldens` rc 0 (944 passed, 4 skipped), no golden file changed (`git status tests/*goldens*` empty); check-diff `[]` before and after. Proof runs: `/work/bench/160-8/t4dp/{A,R2..R16,FINAL}.{lib,cli}.log`.
- Task 5 (1df5fd20): red: T8 `left: Some("dependency") right: Some("primary")`; keys test green before the change. `ci-steps task-affected` rc 0 (3703 passed, 8 skipped); `check-goldens` rc 0 (945 passed, 4 skipped), no golden file changed (the three `primary.json` shards and `fingerprint/shards/` unmoved); check-diff `[]` before and after. Logs `/work/bench/160-8/{t5-*,gate-T5,goldens-T5}.log`.
- Task 6 (81279914): red: T6 kinds `[]`; T6b `left: 0 right: 1`; T7b `no-universe: refused` left 0 right 1 (loaded); T8b diagnostics `[]`. `ci-steps task-affected` rc 0 (3707 passed, 8 skipped; the run before fixed two clippy-dev findings: `if_same_then_else` on the exit gate → one `||` condition, `type_complexity` in T7b → local `type Edit`); `check-goldens` rc 0 (948 passed, 4 skipped), no golden file changed; `cli_b_diff_differential::` green (oracles assert diagnostics, not exit codes). Old-exit-0 reliance: none found (grep of tests, scripts, docs, README, `.github/`: no test, golden or doc expects exit 0 on a fatal preflight; `coverage-strict.*.exitcode.txt` cover the coverage and `--fail-on` gates only). check-diff `[]` before and after. Logs `/work/bench/160-8/{t6-*,gate-T6,goldens-T6}.log`.
- Tasks 5-6 review fixes (79138815): `ci-steps task-affected` rc 0 (3707 passed, 8 skipped); `check-goldens` rc 0 (948 passed, 4 skipped), no golden moved; check-diff `[]` before the commit.
- Task 7 `allowlist.py` (spec §8 Part 0; Task 2 review F1, F2 applied): `S=/work/bench/160-8/allowlist-run; git archive 459294f1 tests/cli-b-goldens | tar -x -C $S/P; git archive 79138815 tests/cli-b-goldens | tar -x -C $S/H; python3 -I /work/bench/160-8/allowlist.py $S/P/tests/cli-b-goldens $S/H/tests/cli-b-goldens` → `149 changed files checked, 301 files on H`, `RESULT: OK`, exit 0 (P = the merge base 459294f1). SHA-256 `cd9590bee37d7bf7956684b7f1d19c1ff2374dd7baea135c5e22fcd87ee69ee5`. It fails on any changed file outside `snapshot/`, `fingerprint/`, `diff/snapshots/` data files and `diff/PROVENANCE.md` (F1, every file type); compares with types and key order (F2); and asserts `.cbor`/`.cbor.gz` (typed) and `.raw.json` (types kept for bool/str, numbers by value, key order free, `undefined` dropped) siblings decode equal on H. Mutation checks (`python3 -I /work/bench/160-8/allowlist-mut.py P H /work/bench/160-8/allowlist-mut`), each exit 2: wrong universe value; missing `apps[].role`; moved `analysisUniverse`; changed unlisted file (`digest/`); changed non-data file in an allowed dir (`fingerprint/*.human.txt`); `true` → `1` (F2); a second PROVENANCE paragraph. Every red test of `## Acceptance tests (red)` is committed (`git grep 'fn <name>' HEAD`; `cli_p1_inventory` is the edited module).
- Task 7 (9cfbe13f): check-diff `[]` before and after. Whole-suite task gate `bash scripts/ci-steps task` on 9cfbe13f: rc 0 (fmt, clippy-dev, gen-syntax, test: 3707 passed, 8 skipped). Log `/work/bench/160-8/gate-task-full.log`.
- Final panel r1 fixes: 4a1fe398, 1054ce5b, 5995fae7 each `task-affected` rc 0 (3707, 3708, 3711 passed) and `check-goldens` rc 0 (948, 948, 951 passed), no golden moved; 86611392 docs only; check-diff `[]` before every commit.
- A8.1 grep re-run at 5995fae7 (`rg -n 'app_guid.*(eq_ignore_ascii_case|to_ascii_lowercase)|eq_ignore_ascii_case\(g\)|eq_ignore_ascii_case\(p\)' src`, final panel G11): `role.rs:37` (the rule), the spec's list (`compiler_oracle.rs:313`, `events.rs:254`, `program_calls.rs:386`, `d13.rs:82-83`, `program_calls_adapter_tests.rs:1505,1641`), the test hits recorded above (`registry.rs:905,912,919`, `projection.rs:510`), and two new hits, neither a role decision: `snapshot_full.rs:617` (the F22 assert: every serialized app is in `dependency_apps`) and `key_classes.rs:924` (`closure_violations`' model-app set, a case fold).
- **Spec §8 Part 1 + Part 2 (CDO identity), after the final panel converged**, through the supervisor (`run --name bench`, exit 0, 368.9 s, queued 0; log `.agent/runs/20261022-220347-220d76/logs/bench/20261023T021940-d984b83c.log`; script and outputs `/work/bench/160-8/measure/`, `RESULT.md`). P = 459294f1, H = b15eb333, rustc 1.96.0, release-fast, the pinned CDO workspace. P == H byte-identical: `analyze --format sarif` `--scope primary` cross-app `6d245ab01d533b26…` (9,803,534 B), `--scope primary --single-app` `2ed3f5133670b69a…` (9,135,441 B), `--scope all` cross-app `08e0e49e044f465c…` (74,069,724 B), `--scope all --single-app` `2ed3f5133670b69a…`; `aldump --program-call-graph-stats` stdout SHA-256 `c032c8db56f1fd070fc4b7e4f1971a73019a4c8569db81a6aaeb7e0c1a1594ef` both sides; `digest --routine HasBasicPermission --format json --deterministic` `8ff85913ef306e4f…`; `query effects` same routine `190e22a7ba02b025…`. F8 not fired. Part 2: `fingerprint --format json --deterministic` 129,612,533 B (P) → 129,612,598 B (H); `allowlist.py` over it (placed as `fingerprint/cdo.json` in two trees) `RESULT: OK`, exit 0. Cross-app gap census not re-run (0 on CDO by P5).
- **Step 11 full gate set** on H = `b15eb3335c77ea85360628834f8a608bfacc3441` (B = `459294f18703b06a371d351bd95871e6f2314e99`; origin/master unmoved since rebase 1, so no second rebase and no range-diff), all supervised, run in sequence from the worktree, `CDO_WS` exported from `.agent/config.json` (pinned CDO workspace), `cdo-gate` called with no argument:
  - `check-goldens-coverage` (`bash scripts/check-goldens --verify-coverage`): exit 0, "coverage OK (36 golden dirs declared)". Log `.agent/runs/20261022-220347-220d76/logs/check-goldens-coverage/20261023T022639-17299af5.log`.
  - `ci-steps-all` (`bash scripts/ci-steps all`): exit 0, 296.6 s. Log `…/logs/ci-steps-all/20261023T022640-6ff283e4.log`.
  - `check-goldens` (`bash scripts/check-goldens`): exit 0, 0.1 s: the script's own tree-stamp skip ("this exact tree passed 'scripts/ci-steps test'"), the stamp written by the `ci-steps all` run just before on the same tree. Log `…/logs/check-goldens/20261023T023136-a00214bb.log`.
  - `cdo-gate` (`bash scripts/cdo-gate`): exit 0, 263.1 s; last suite `2254 passed; 0 failed; 4 ignored`. Log `…/logs/cdo-gate/20261023T023137-90f25f96.log`.
  - Toolchain rustc 1.96.0; grammar `tree-sitter-al` main `960ed312` (committed `node-types.sha256`).
- `allowlist.py` after final panel F2 (SHA-256 `e12252c9e3af8491e52d948d6f4826ec540c5d0dc90b5d24a3d56f5be2d1ceda`): every snapshot-shaped file on H (raw snapshot, `capability-snapshot` envelope, shard manifest) is validated whether or not its bytes changed, and a changed data file of any other shape fails. P (459294f1) vs H (86611392): `149 changed files checked, 301 files on H`, `RESULT: OK`, exit 0. P vs P: exit 2 (schema 3 everywhere). Mutations: the 7 above plus three restore-to-P cases (`.raw.json`, `.envelope.json`, a diff input), all exit 2.

## Discoveries

Filed by `/orchestrate` step 10 (the entries below this block are the narrative; the fatal-preflight exit was fixed in scope, D5.4, and the T9b/T9d arms are carried to epic step 6 by spec §12, so neither is filed):

```json
[
  {"subsystem": "l5 detectors (d1 actionable anchor)", "locator": "src/engine/l5/detectors/d1.rs detect_d1 role_by_routine (every routine \"primary\") passed to pick_actionable_anchor", "symptom": "In cross-app runs d1 builds its anchor role map with every routine marked primary, so a d1 finding whose path ends in a dependency routine can take a dependency step as its actionable anchor and survive --scope primary (registry role_scope_and_sort). #220 kept the map unchanged because converting it to the one role rule (Model::routine_role) would move CDO SARIF.", "kind": "bug", "origin_issue": 220, "pre_existing": true, "reproducer": "Cross-app analyze on the pinned CDO workspace, --scope primary: compare d1 findings with the map built from Model::routine_role instead of the all-primary map; any finding that drops is the shape.", "capability": "d1's anchor map built from the one role rule (Model::routine_role), with a CDO triage of every d1 finding it moves", "acceptance": "d1.rs builds role_by_routine from Model::routine_role; a hand-stated test with a dependency terminal routine fails under the all-primary map; CDO SARIF moves are triaged and recorded"},
  {"subsystem": "l5 snapshot (callsite evidence vs resolution ledger)", "locator": "src/engine/l5/snapshot.rs derive_callsite_resolutions vs derive_callsite_evidence; key_classes.rs closure F25 exception", "symptom": "The callsite-resolution ledger has a row per syntactic call site, but callsiteIndex keeps only call sites a fact witnesses or a typed edge anchors, so an unresolved site with resolvedEdges [] names a callsiteId that has no callsiteIndex row (a dangling reference on the unfiltered composition). #220's closure check allows exactly that one shape (F25).", "kind": "bug", "origin_issue": 220, "pre_existing": true, "reproducer": "tests/r2-5b-fixtures/cross-app-resolution composed with SnapshotUniverse::SingleApp over the cross-app model: callsiteResolutions rows with status external and resolvedEdges [] whose callsiteId is absent from callsiteIndex (4 rows).", "capability": "every callsiteResolutions.callsiteId has a callsiteIndex row (derive_callsite_evidence keeps every site the ledger names), and the F25 closure exception is removed", "acceptance": "closure_violations reports no callsite-domain violation on the r2-5b fixture with the F25 exception deleted; goldens moved by the extra rows are triaged"}
]
```

- **F25: an unresolved call site's resolution row dangles on the base** (Task 4). The ledger
  (`derive_callsite_resolutions`) has a row per syntactic call site, but
  `derive_callsite_evidence` keeps only call sites a fact witnesses or a typed edge anchors.
  R5B: `callsiteResolutions.callsiteId` `1dc54afacf55e729/3548…1247/cs1`, `/cs2`, `/cs4`
  and `…/31ac…08e0/cs1` (status `external`, `resolvedEdges: []`) have no `callsiteIndex` row,
  measured on the UNFILTERED composition (`SnapshotUniverse::SingleApp` over the cross-app
  model), so pre-existing. Exactly that shape (a `callsiteId` of a resolution row with empty
  `resolvedEdges`) is the callsite domain's one external shape (`key_classes.rs`, comment
  names F25). The committed cli-b goldens have 0 such rows.
- **T9b and T9d positive arms are vacuous at this base** (Task 4, same root cause as
  deviation 1): `CallChain` has no capability fact in memory on the cut (its facts come
  through `Outer -> Inner`, cone-only). T9b asserts that base truth and checks `CallChain`'s
  `typedEdges`/`callsiteResolutions` rows and every primary fact instead; restore "CallChain's
  facts stay" at step 6. T9d break 4 is unobservable on composed trees (every primary Evidence
  row is referenced by its own routine's fact or edge); pinned on typed rows by
  `evidence_keeps_unreferenced_primary_rows`.

- **A fatal diff preflight exits 0** (P9 extra). `run_diff_engine` returns empty findings on a
  fatal preflight (`gate/diff/mod.rs:145-153`) and `DiffEngineResult` carries no fatal bit, so
  `diff/cli.rs:299-307` exits 0 even under `--fail-on`. Measured on the base binary: two vendored
  inputs of different apps → `app-identity-mismatch` printed, exit 0. A CI job that diffs the
  wrong pair passes. Step 8's mixed-universe refusal would inherit it; the spec takes the fix in
  scope (D5). If the panel moves it out, it becomes its own issue.
- **d1's actionable-anchor map is all-primary in cross-app runs** (spec panel O1). `detect_d1`
  builds `role_by_routine` with every routine `"primary"` (`detectors/d1.rs:1993-1998`) and
  passes it to `pick_actionable_anchor` (`:620`, `:1572`). Cross-app, a d1 finding whose path
  ends in a dependency routine can get a dependency step as its actionable anchor and so
  survive `--scope primary` (`registry.rs:727-745`). Converting the map would move CDO SARIF,
  so step 8 keeps it; it needs its own issue with a CDO triage.

## Not delivered

- Nothing from the acceptance matrix. Spec §8 Parts 1-2 (CDO identity: SARIF ×4, histogram
  SHA, digest, `query effects`; the CDO fingerprint through `allowlist.py`) and the branch gate
  (`ci-steps all`, `cdo-gate`) are recorded green under `## Gate results`.
- Final panel C6 (deferred to step 9, spec F16): `filter_by_class` converts the typed rows to
  CBOR to read their references, then the composer converts them again. Cross-app only, which
  no command composes before step 9; step 9's peak and time bounds own it.
- Whole-suite task gate after the final panel r1 fixes: `bash scripts/ci-steps task` on
  86611392, rc 0 (fmt, clippy-dev, gen-syntax, test: 3711 passed, 8 skipped). Log
  `/work/bench/160-8/gate-task-final-r1.log`. (Recorded here, at the end, so the register's
  ledger anchors do not shift.)

### Final panel r2: discrimination proofs and gates (appended at the end for the same reason)

| test | mutation | fail output | pass output | commit |
|---|---|---|---|---|
| F6 r2: `every_row_variant_field_is_classified` (new; every `SnapValueSource`, `SnapTempState`, `SnapCapabilityExtra`, `SnapshotGraphEdge` variant, all optionals set, plus Evidence, resolution, branch frame, `inputsMetadata`, root classification, event declaration, coverage, gap) | red before the list fix: `resourceArgSource.index`, `extra.modal`, `extra.includeSender`, `extra.tempState.parameterIndex`, `typedEdges.candidateCount`, `targetIdSource.index`, `inputsMetadata.rootsConfigIgnored` (+ nested) unlisted; break after: `key_classes.rs` `"modal"` → `"modalDropped"` | `snapshot_full.rs:3384` "unclassified fields: {capabilityFacts.extra.modal}" | ok | 225fb0a6 |
| G5 r2: `single_app_universe_binds_a_single_app_model` second arm (DD cross-app model, every dependency routine removed, dependency objects/tables remain, asserted) | `snapshot_full.rs` guard checks routines only (`true \|\| …` on objects and tables) | `:1916` "a cross-app model composed as SingleApp must panic: Map({…})" | ok | 225fb0a6 |

- 225fb0a6: `task-affected` rc 0 (3712 passed, 8 skipped); `check-goldens` rc 0 (951 passed), no golden moved; check-diff `[]` before.
- b15eb333 (docs only, G7): check-diff `[]` before and after. Whole-suite `bash scripts/ci-steps task` on b15eb333: rc 0 (3712 passed, 8 skipped), log `/work/bench/160-8/gate-task-final-r2.log`. Register: round-6 marks recorded (opus 29 accepted; sol 26 accepted, F6/G5/G7 re-raised then refixed and set unreviewed); nine anchors moved by line only (hash unchanged); all 78 anchor hashes re-checked.
