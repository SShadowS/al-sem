# Issue #83 — numberless objects (every Interface and ControlAddIn) share the object id `<app>/Interface/0`

## Claim

```json
{"run_id":"20261023-142104-a7c0a4","issue":83,"attempt":1,"session":"claude-code-session:e2bf58b3-5d1b-467f-8c3b-1c59a8d42f0c","branch":"issue/83-agent-discovery-engine-ids-obje-a1","worktree":"/work/al-sem-issue-83-a1","body_hash":"c97cf38ff6af18e5589fe27bdfdc802cc022d1820a62bb6f79ef27f22ec3471f"}
```

Issue body sha256 `c97cf38f…3471f` (from the claim). Worktree from `origin/master` @
`3a2d5923` (2026-10-23), grammar via `TREE_SITTER_AL_PATH=/work/alsem/tree-sitter-al`. The work
below up to "FINAL PANEL CONVERGED" is the 2026-10-15 attempt (run `20261015-195811-d98789`,
worktree from `662f5137`, open PR #114); its commits are replayed onto `3a2d5923` (see the
Timeline's 2026-10-23 entries).

Reviewer roster (operator instruction, 2026-10-15): `gpt-6.1-sol` fills the `astra` slot (`sol`)
and `claude-opus-5.5` fills the `flash` slot (`opus`); attested with `--substitute astra=sol
--substitute flash=opus`.

## Classification

**ARCHITECTURAL.** The object-id scheme is shared by eight producers (R0, R1a, the model,
`model_object_id`, the dependency ABI projection) and by the internal and stable routine ids built
on them. The fix changes the encoder's signature and `CanonicalRoutineKey`, and moves goldens. Spec:
`docs/superpowers/specs/2026-10-15-issue-83-numberless-object-ids-design.md`.

## Assumption probes

| probe | result | evidence |
|---|---|---|
| P1 — the reproducer holds | **HOLDS**: `tests/r0-goldens/ws-interface-dispatch…` has `…:Interface:0` for both `IEmpty` and `IProcessor` | golden lines 34, 40 |
| P2 — where the id is minted | one encoder (`ids.rs:130`), eight producers, all `unwrap_or(0)` | `$SCRATCH/83/map.md` §1 |
| P3 — numberless kinds | source: Interface, ControlAddIn (+ a numbered kind whose number did not parse); deps: Interface, ControlAddIn, DotNetPackage | map §2 |
| P4 — routine ids | internal hashes the NUMBER; stable embeds the stable object id; dependency ABI interface methods are routines, so they collide too (beyond the issue's statement) | map §1, §4 |
| P5 — #71 ids | fold the object name in as the member (`workspace.rs:899-931`) | map §5 |
| P6 — consumers split ids on `/`, `:`, `#`, `::` | a raw name in the id would break `abi_rows.rs:110-116` and `selector_index.rs:22,114`; so the name is percent-encoded behind `@` | map §3 |
| P7 — goldens holding a numberless id | `tests/r0-goldens` 6, `tests/r1a-goldens` 6; no ControlAddIn/DotNetPackage id in any golden | map §7 |

## Timeline

- Claimed (attempt 1, tick 5 of the `/orchestrate 10 ticks` loop; ranked first, value 3), worktree
  created. Mapping every object-id producer and consumer (`$SCRATCH/83/map.md`).
- Probes P1-P7; classified ARCHITECTURAL; spec written.
- Spec round 1 (sol + opus): sol 4 important; opus 4 important + 6 minor. All source-checked:
  - the ABI zero sentinel (`symbol_reference.rs:267-268`, `abi_ingest.rs:421-425`);
  - the golden regex hits outside r0/r1a are arity and index suffixes;
  - DotNetPackage → `Codeunit` (`abi_ingest.rs:259-278`).

  The spec was rewritten: D2a ABI zero rule; D5 duplicate-checking harness; tests T3/T3b/T5/T5b/T6;
  sort contract; Limits; upgrade note; CDO/DO measurement. Register R1-01…R1-13, all fixed.
- Spec round 2: opus accepted all 13, ready, plus 3 minors (R2-02…R2-04). sol re-raised R1-08 (T5b
  observable; kind mismatches) and R1-09 (`@` vs digit ordering), and its R1-08 point is R2-01. All
  fixed.
- Spec round 3 (`spec_rounds` 2 left): both accepted all 17, no new findings, "ready". **SPEC
  CONVERGED.**
- "Before" release-fast `alsem`/`aldump` built from `662f5137` and copied to `$SCRATCH/83/before/`,
  for the CDO/DO measurement.
- Before-measurement on CDO (the pinned baseline, `bc3ccb18`). The day-to-day DO checkout
  is at the same SHA, so DO is the same input and is not measured twice. With the "before" binaries:
  - `fingerprint --format json --deterministic`: the primary app's numberless ids appear as ONE
    `…:Interface:0` (8 occurrences) and ONE `…:ControlAddIn:0` (4 occurrences), so every interface
    and every control add-in of the app collapses onto one id;
  - `analyze --format json --deterministic`: 2293 findings (`--scope primary`), 26954 (`--scope
    all`).

  Outputs are in `$SCRATCH/83/{fp,an,an-all}-before.json`.
- Task 1 dispatched to an opus implementer (`$SCRATCH/83/impl-t1.md`).
- The golden-regen hook needed a receipt in the main checkout's `.agent/`, which the implementer may
  not touch. The conductor wrote it after checking the reported non-regen divergences: R1a has 6,
  all `:Interface:0` → `:Interface:@<name>`, and R0 stops at the new D5 duplicate guard.
  - The hook was then changed, at the operator's request, to also accept a receipt in the worktree.
    That change also gates regens run through `agentflow run … --`, which were never gated before.
- Task 1 is commit **`893c0ef3`**:
  - RED/GREEN and every proof are in `$SCRATCH/83/impl-t1.md`.
  - `--lib` 2034, `l2_ir` 73, `cli` 354, `r25_abi` 9 and `differential` 12 pass; clippy clean;
    `check-goldens` green.
  - Goldens: 10 files, 12 lines, every one a numberless `stableObjectId` (`:0` → `:@<name>`). No
    other family moved.
  - Deviations: the ABI zero rule appeared 5 times, so it is folded into
    `AbiRoutineKey::object_node_key()`; one source helper, `ir_walk::ir_object_key`; the P7 table
    declaring-object id uses the key; one extra #71 test pin updated.
- Spec and plan committed: `76e5ae63`.
- Task 1 review (sonnet) and the new `id-shape-auditor` dispatched in parallel. After-binaries built
  and measured with the `cdo-measure` skill.
- **CDO measurement**, before `662f5137` vs after `76e5ae63`; same workspace SHA `bc3ccb18`,
  dirty 0 on both sides. Output in `$SCRATCH/83/measure/`, `comp-events.txt`.
  - `analyze --scope primary`: 2293 → 2293 findings; fingerprints 0 added, 0 removed, 2286 kept.
  - `analyze --scope all`: 26954 → 26954 findings; fingerprints 0 added, 0 removed, 26926 kept.
  - `fingerprint` output, Interface ids: 1 distinct (`…:Interface:0`, 8 occurrences) → 7 distinct
    (`@cdo%20edocument%20dispatcher`, `@cdo%20module%20license`, …; 14 occurrences).
  - ControlAddIn ids: 1 distinct (`:0`) → 3 distinct (`@cdo%2Eeditor`, `@cdo%2Eprintservice`,
    `@cdo%2Eprintservicecallback`).
  - `/0/event/`: 0 → 0. `/event/`: 41693 → 41693, unchanged, so no event id moved (R1-06; the
    expected 0 holds).
  - No `:Codeunit:@` and no `:DotNetPackage:` ids on either side: no recovered codeunits and no
    DotNetPackage objects in the primary app.
  - So the change makes the CDO primary app's 7 interfaces and 3 control add-ins distinct, and moves
    no finding.
- Task 1 review (sonnet, `$SCRATCH/83/review-t1.md`): **changes-requested**.
  - Important: the fingerprint object part (`fingerprint.rs:152`) has no pinning test.
  - Minors: no `object_key` = last-segment check for source objects; ControlAddIn not exercised; the
    `"{type}/{number}"` lookup indexes (`combined_graph.rs:487`, `symbol_table.rs:49,96`) still share
    `type/0`; the empty name argument at `mod.rs:608`.
- `id-shape-auditor` (first use; `$SCRATCH/83/id-audit-t1.md`): **safe**, no reader breaks.
  - Readers that now work correctly: the identity table, contracts, detector maps, routine maps and
    `r3a4`.
  - Readers that change as intended: the fingerprint object part, and the #71 ids, which move.
  - Non-blocking items:
    - d33's object-id vs table-id join no longer matches a Table whose number did not parse
      (non-compiling input). Recorded as a limit.
    - The M8 comment is not yet rewritten (Task 2).
    - Stale docs in `fingerprint.rs`, `r3a4_projection.rs` and `finding.rs`.
    - Five inline copies of the ABI zero rule remain.
  - Predicts ADDED rows in CDO-pinned outputs (dependency interfaces no longer collapse). `cdo-gate`
    decides.
- Review-fix round (sonnet; `task_attempts` task-1: 1 left; `$SCRATCH/83/fix-t1.md`):
  - fingerprint pin test;
  - source `object_key` = last segment;
  - ControlAddIn in T3;
  - the five ABI zero-rule copies use `abi_object_number`;
  - stale docs and the `mod.rs:608` comment.

  Recorded and not changed: the `type/number` lookup indexes, and d33 for a table without a parsed
  number. Both are pre-existing and about non-id lookups or non-compiling input.
- Review fixes are commit **`67c37b67`** (`$SCRATCH/83/fix-t1.md`):
  - fingerprint pin test, with a proof (`fingerprint.rs:418` fails when broken);
  - `object_key` = last segment for every source object of T3/T3b (proof at `:164`, `:225`);
  - ControlAddIn `"My Add In"` asserted from P1/P2/P3/P5 (proof at `:140`);
  - the five ABI zero-rule copies now call `abi_object_number` / `object_node_key`;
  - stale docs fixed.

  Results: `--lib` 2035, `cli` 354; clippy clean; no golden moved; check-diff `[]`.
- Task 2 dispatched (opus; `$SCRATCH/83/impl-t2.md`): T6, T7, the M8 comment, the CHANGELOG.
- Task 2 is commit **`bfa430e7`**:
  - T6 (full snapshot keeps both interfaces with distinct `stableId`s and their own names) and T7
    (identity table ordinal `…:1` < `…:@a`, contracts `locale_compare` `…:@a` < `…:1`).
  - Proofs: T6 with `object_key(None)` → `"0"` fails at `:151`; T7 with the identity table sorted
    by locale fails at `:194`; T7 with the contracts sorted ordinally fails at `:202`.
  - M8 comment rewritten (code unchanged); CHANGELOG entry.
  - Results: `--lib` 2035, `cli` 356; clippy clean; `check-goldens` green, no golden moved;
    check-diff `[]`.
  - Deviation: T6 asserts the identity-table display name the model already carries (`Ø ""Q"" /:#%@
    If`, raw doubled quote) → discovery D1.
- Task 2 review (sonnet, `$SCRATCH/83/review-t2.md`): **approve**, with 2 minors, both fixed by the
  conductor in **`18d6dd59`**:
  - the per-site `M8 … single-case … ordinal-safe` notes are now marked historical in the M8
    module doc (a key holding a stable id is no longer single-case);
  - the CHANGELOG list of collapsing maps is narrowed to what tests pin.

  Hook goldens OK; check-diff `[]`.
- Gates started on `18d6dd59`. `code-review high` (built-in skill, forked) and the final panel
  round 1 (sol + opus) are running in parallel.
- Final panel round 1 (`final_rounds` 4 left):
  - opus: all 17 accepted, ready, plus 2 minors (graphify id; CHANGELOG wording and the d33 limit).
  - sol: 15 accepted. It re-raised R1-05 (no cross-app logical-identity count) and R1-06 (substring
    counts do not prove event ids are equal), plus 1 minor (`projection.rs` comments).
  - `code-review high`: 9 findings.

  Gates on `18d6dd59` were stopped before any verdict (TaskStop, and the orphan `ci-steps build`
  tree killed).
- Final-panel fixes are commit **`e8cb5a9c`** (opus; `$SCRATCH/83/final-fix.md`):
  - fixed: graphify id; `type/number` lookups skip numberless objects; `ObjKey::from_abi` at every
    copy that builds an ObjKey; the LSP URI Id segment; `model_object_id`'s `n.to_string()`;
    `reached` computes the id once; the per-site M8 notes; the projection comments; the CHANGELOG;
  - 4 proofs; `--lib` 2038, `cli` 356, `l2_ir` 73, `r25_abi` 9, `differential` 12; clippy clean;
    no golden moved; check-diff `[]`.
  - Cross-app CDO probe (temporary, deleted; `$SCRATCH/83/probe-crossapp.txt`):
    - 172 numberless objects in 14 (app, kind) pairs: rows = ids = logical identities = 172; the
      old scheme gave 14 ids; 0 residual collisions;
    - 3622 bound subscriptions, 0 with an `ObjKey::Name` publisher; 0 event ids with `/@`.

  Deferred: CR1/CR2, the source `ObjKey::Name` raw-vs-logical name, as discovery D2. Refuted: CR4,
  the D5 duplicate panic, which is by design. Register F1-01…F1-12.
- Gates rerun on `e8cb5a9c`; final panel round 2 sent.
- Final panel round 2 (`final_rounds` 3 left):
  - opus: all 29 accepted, ready, plus 1 minor (F2-01: the graphify CHANGELOG line over-claims). The
    conductor also found that the line names the wrong command (`alsem graph` instead of `aldump
    --graphify-export`).
  - sol: 28 accepted, re-raised F1-01 (graphify's `sanitize_slug` is lossy, so `I Foo`/`I-Foo`
    still share an id).
- The gates on `e8cb5a9c` were stopped before any verdict (orphan `ci-steps build` tree killed).
- Fixed at the root in **`10809ef8`** (conductor):
  - `obj_key_str` keys every `ObjKey::Name` with `engine::ids::object_key(None, name)`, so the
    `al:obj:`, `al:rtn:` and `al:abi:` graphify ids never slug a numberless object's name.
  - The test gains collision cases.
  - RED at `graphify_export.rs:1543`; proof: keeping the `@` but slugging the name fails at
    `:1550`; restored, 10 pass.
  - `--lib` 2038; clippy clean; hook goldens OK; check-diff `[]`; CHANGELOG reworded.
- Gates rerun on `10809ef8`; final panel round 3 sent.
- Final panel round 3 (`final_rounds` 2 left): sol and opus both accepted all 30, no new findings,
  "ready". **FINAL PANEL CONVERGED.**
- The 2026-10-15 run stopped only on `ci-claude-review-error` (a CI tooling failure); PR #114 stayed
  open.
- **2026-10-23, replay (run `20261023-142104-a7c0a4`).** Priority: the bug is live in CentralGauge's
  demo (GitHub **#245**: a new interface is reported as a critical `procedure-signature-changed`,
  because every interface is `<appId>:Interface:0`). The PR body will carry "Fixes #245".
  The 7 code and doc commits were cherry-picked onto `3a2d5923`, each message ending with a
  `Replayed onto 3a2d5923 …; conflicts: …` line:
  - `8e6f09f7` (was `893c0ef3`): `l2_workspace.rs` — master had folded the id into
    `stable_object_id_ir`, so the object key moved into that helper; `tests/cli/main.rs` module
    list. Adapted to master's APIs: the test's `InputSet` reads (#160), the cross-app graph from a
    `FULL` build (`build_cross_app_program` is gone), and `object_key` in two new test
    `ModelObject`s (`effect_query_cli.rs`, `role.rs`). Goldens applied cleanly.
  - `b8b19a6e` (was `76e5ae63`): none. `51fc9cea` (was `67c37b67`): none.
  - `aea9a4db` (was `bfa430e7`): CHANGELOG (entries moved to `[Unreleased]`), the test file
    (`FullSnapshotOptions` now takes `inputs` and `universe`, #220).
  - `c9468ddd` (was `18d6dd59`), `e21a92c8` (was `10809ef8`): CHANGELOG only.
  - `c9418408` (was `e8cb5a9c`): CHANGELOG; `combined_graph.rs` (master's `trigger_fires` test kept
    beside the #83 test); `snapshot.rs` (the M8 note merged with master's guid-casing note).
- **T8** (`4a620b57`), a new `alsem diff` regression test: old tree `IFoo` with `Run()`, new tree
  adds `IBar` with the same `Run()`, both file orders, for Interface and ControlAddIn.
  - RED on master `3a2d5923` (master's debug `alsem`, `/work/bench/83/t8-red-master-*.json`): when
    the new file sorts last, one finding, `procedure-signature-changed`, critical, on
    `…:Interface:0` (`…:ControlAddIn:0`), named `IBar`. When it sorts first: no finding at all.
  - GREEN: the only finding is `object-added`, info, `…:Interface:@ibar`.
- **T9** (`ebf9af82`): #245's repro verbatim as `tests/fixtures/ws-diff-interface-added`
  (`start/`, `final/`), run as CentralGauge does: `alsem fingerprint` of each, then `alsem diff
  start.json final.json`.
  - RED on master (`/work/bench/83/red-245/`): both fingerprints hold only
    `11111111-…:Interface:0`; the diff's one finding is a critical `procedure-signature-changed`
    named "Repro Transport".
  - GREEN: the stable ids `…:Interface:@repro%20strategy` and `…:Interface:@repro%20transport`;
    the one finding is `object-added`, info, "Repro Transport".
  - `FIXTURE_ROOTS` `fixtures` 34 → 36 (the two new app dirs).
  - The CHANGELOG states the stable-id shape change of numberless objects, why, and that stored
    baselines see those ids move once (CentralGauge has none yet).
- Goldens: `check-goldens` with no regen on `ebf9af82` was already green (995 passed): the 10
  files / 12 lines the replayed `893c0ef3` moved (`:Interface:0` → `:Interface:@<name>`, r0 and
  r1a) applied cleanly and are still what the engine produces. Receipt
  `.agent/golden-triage.md`; `check-goldens --regen` then changed nothing; `check-goldens` green.
  Same files and lines as the 2026-10-15 triage. Logs in `/work/bench/83/`.
- `scripts/ci-steps task` green on `ebf9af82` (3832 passed, 8 skipped;
  `/work/bench/83/ci-steps-task.log`); check-diff `[]` before and after every commit.
- Register re-anchored to the replayed text (`path:L1-L2` + sha256 of those lines). 20 entries keep
  their marks (anchored text byte-identical to what the round reviewed); 10 are reset to
  `unreviewed` (R1-03, R1-09, R1-11, F1-06, F1-08, F1-09, F1-10, F1-11, F1-12, F2-01).

## Acceptance matrix

| Acceptance (issue #83) | proof |
|---|---|
| unnumbered objects get a unique object id (keyed by name) | `object_key` (`@` + percent-encoded folded logical name); T1 vectors; R0/R1a goldens regenerate with distinct ids; CDO cross-app probe: 172 numberless objects, 172 distinct ids (the old scheme gave 14) |
| two interfaces get distinct `ModelObject` ids | T3 (`IEmpty`, `"Ø ""Q"" /:#%@ If"` + a ControlAddIn) from P1/P2/P3/P5; T6 end to end |
| #71's declaration-only id is re-derived from it | D4 (member `None`); T4 shape `…:Interface:@<name>#` + 64 hex |
| a test pins both object ids | T3 (literal ids for both interfaces) |
| (found) dependency interfaces, recovered codeunits, fingerprints, graphify and lookups no longer collide | T5, T5b, T3b, the fingerprint pin test, the graphify test, and the two `type/number` lookup tests |

## Discrimination proofs

| test | break | fail | commit |
|---|---|---|---|
| T1 `object_key_vectors` / `…never_holds_a_separator` | `None` → `"0"` | `ids.rs:692`, `:713` | `893c0ef3` |
| T3 | P3 skips the unescape | `cli_numberless_object_ids.rs:113` | `893c0ef3` |
| T3b (+ join) | P2 `unwrap_or(0)`; P3 `Name(_) => "0"` | `:178`; `:228` (not-found) | `893c0ef3` |
| T4 (+ 2 #71 tests) | member `Some(name)` again | `cli_digest_changed_spans.rs:1823`, `:2256`, `:2220` | `893c0ef3` |
| T5 / T5b | P8 `Some(n)`; `dep_object_to_l3` `object_key: ""` | `:251`; `:315` | `893c0ef3` |
| D5 duplicate / name | duplicate row in the golden; name changed in the golden | `differential.rs:284`; `:379` | `893c0ef3` |
| fingerprint object part | object part back to `object_number` | `fingerprint.rs:418` | `67c37b67` |
| source `object_key` = last segment | `ModelObject.object_key: String::new()` | `cli_numberless_object_ids.rs:164`, `:225` | `67c37b67` |
| ControlAddIn | `ir_object_key` → `"0"` for ControlAddIn | `:140` | `67c37b67` |
| T6 | `object_key(None)` → `"0"` | `:151` | `bfa430e7` |
| T7 identity table / contracts | identity table sorted by locale; contracts sorted ordinally | `:194`; `:202` | `bfa430e7` |
| `type/number` lookups (2 tests) | guard `!= 0 \|\| true` | `symbol_table.rs:157`; `combined_graph.rs:1084` | `e8cb5a9c` |
| LSP URI Id segment | segment back to `object_number` | `handlers.rs:1321` | `e8cb5a9c` |
| graphify id (injective) | the slug; then `@` + slug | `graphify_export.rs:1543`; `:1550` | `10809ef8` |
| T8 `diff_adding_an_interface…` / `…_a_controladdin…` | `object_key(None)` → `"0"` | `cli_numberless_object_ids.rs:597`: `procedure-signature-changed`, critical, `…:<Kind>:0`; reverted → 8 pass (`/work/bench/83/t8-discrimination-broken.log`) | `4a620b57` |
| T9 `diff_issue_245_interface_added_fixture` | `object_key(None)` → `"0"` | `:674`: both ids `…:Interface:0`; reverted → 9 pass (`/work/bench/83/t9-discrimination-broken.log`) | `ebf9af82` |

## Findings register summary

30 entries: spec R1-01…R1-13, R2-01…R2-04; final F1-01…F1-12, F2-01.
- 27 fixed.
- 2 deferred, both minor (F1-04/F1-05: the source `ObjKey::Name` raw-vs-logical name → discovery
  D2).
- 1 refuted (F1-07: the D5 duplicate panic, which is by design).

Every entry was accepted by sol and opus (final round 3, 2026-10-15). After the 2026-10-23
replay, 10 entries whose anchored text changed are `unreviewed` again (see the Timeline).

## Gate results

Replay (2026-10-23), H = `ebf9af82`, B = `3a2d5923`: `scripts/ci-steps task` 0;
`check-goldens --regen` 0 (no change); `check-goldens` 0; check-diff `[]`. The branch gates
(`ci-steps all`, `cdo-gate`) have not run on the replay yet.

The 2026-10-15 gates, on H = `10809ef829791c609a23d641885f148d733b1642`, with B = `662f5137a54253496dedb2c81277f639492f2ffc`.
`origin/master` has not moved, so there was no rebase. Toolchain rustc 1.96.0; grammar `8682cb5f`;
CDO_WS = the pinned baseline (`bc3ccb18`).

| gate | exit | seconds | log |
|---|---|---|---|
| ci-steps-all | 0 | 653.7 | `logs/ci-steps-all/20261015T222147-1de83074.log` |
| check-goldens-coverage | 0 | 0.3 | `logs/check-goldens-coverage/20261015T223242-8e67a128.log` |
| check-goldens | 0 | 72.7 | `logs/check-goldens/20261015T223242-cdb8a1c9.log` |
| cdo-gate | 0 | 208.2 | `logs/cdo-gate/20261015T223355-92f668e1.log` |

Every gate ran supervised. After the gates, `git status --porcelain -- . ':!.agent'` was empty. The
CDO-pinned outputs did not move.

## Not delivered

- The source `ObjKey::Name` raw-vs-logical normalisation (D2), and display names keeping `""` (D1).
  Both are pre-existing and filed as discoveries.
- Table ids for a table whose number does not parse (`encode_table_id`), and the d33 join on such a
  table. These are non-compiling input, stated in the CHANGELOG Limits.
- Graphify's routine segment is still a lossy slug (`"Do-It"` / `Do_It` of one arity share an
  `al:rtn:` id); existing behaviour, deferred from CR4.

## Discoveries

```json
[
  {
    "subsystem": "model / object display name",
    "locator": "src/program/model/workspace.rs:786 (ModelObject.name = o.name, the raw IR identifier); shown by the identity table (src/engine/l5/snapshot.rs derive_identity_table) and contracts",
    "symptom": "A quoted object name with an escaped inner quote keeps the escape in every display: interface \"Ø \"\"Q\"\" If\" is shown as `Ø \"\"Q\"\" If` instead of its logical name `Ø \"Q\" If`. The id (since #83) and the #71 routine ids use the unescaped logical name, so the display and the id disagree.",
    "kind": "bug",
    "origin_issue": 83,
    "reproducer": "tests/cli/cli_numberless_object_ids.rs T6 (full_snapshot_keeps_both_numberless_interfaces) asserts the raw name; any workspace with `interface \"A \"\"B\"\" C\" {}` shows `A \"\"B\"\" C` in `alsem fingerprint --format json` identities.",
    "pre_existing": true,
    "capability": "object display names are the logical AL name (inner doubled quotes unescaped), consistent with the ids",
    "acceptance": "ModelObject.name (or the display projection) unescapes `\"\"` to `\"`; the identity table and contracts show `A \"B\" C`; T6's display-name assertion is updated to the logical name; goldens with an escaped quoted object name (if any) move only in that field."
  },
  {
    "subsystem": "program graph / object identity (ObjKey::Name)",
    "locator": "src/program/node_extract.rs:645-647 (and graph.rs:229,318; resolve/index.rs:1188; resolve/receiver.rs:3759,3885) key a source numberless object by fold_identifier(RAW name, inner \"\" kept); src/program/abi_ingest.rs:420-425 keys the ABI object by fold(LOGICAL name); src/program/model/mod.rs model_object_id unescapes every ObjKey::Name",
    "symptom": "The graph has two spellings for one object name: source keys keep the escaped `\"\"`, ABI keys hold the logical `\"`. model_object_id's unescape is right for source nodes and wrong for an ABI node whose logical name contains `\"\"` (source `\"A\"\"\"\"B\"`): there the model's object id (P3) differs from the dependency projection's (P8), so program_calls' `reached` join and event publisher ids miss for that object. One object reached as dependency source and as ABI also gets two different ObjKeys.",
    "kind": "bug",
    "origin_issue": 83,
    "reproducer": "A symbol-only dependency declaring `interface \"A\"\"\"\"B\"` (logical name A\"\"B, no Id): projection id `.../Interface/@a%22%22b`, model_object_id `.../Interface/@a%22b`.",
    "pre_existing": true,
    "capability": "every ObjKey::Name holds the folded LOGICAL name, on source and ABI alike, so ids built from the graph and from the projection agree for every name",
    "acceptance": "Source ObjKey::Name is built from unescape_al_identifier(name) at every construction and lookup site (one helper); model_object_id stops unescaping; a test with a dependency name containing `\"\"` pins P3 == P8; resolution on CDO unchanged (cdo-gate)."
  }
]
```

## Round 9 — final panel round 1 on the replay (2026-10-23)

- sol and opus accepted all 10 reset entries and stand by the 20 carried marks: every one of the
  30 is `accepted` again. New: NF1 (sol), NG1, NG2 (opus), CR1-CR9 (`code-review` at high).
- Fixes are commit **`a83bbc43`**: NF1, NG1/CR9, NG2, CR2-CR8 fixed; CR1 deferred (conductor
  decision: no schema bump, since CentralGauge has no stored baselines; the CHANGELOG states the
  one-time critical removal and the re-mint remedy).
  - CR2: nothing reads the `al-preview:` `Id` segment (`parse_al_preview_uri` skips it), so it
    is master's value again (`0` for a numberless object), pinned by the URI test.
  - CR3: the dead "two interfaces share `/Interface/0`" claim is removed from the #78 tests.
  - CR5 narrows F1-07: an `#if`-split object (one id, one name) is accepted; two names panic.
  - CR6/CR7/CR8: `SymbolTable` lookups, `ObjKey::raw_text`, lazy `ObjKey::from_abi`;
    `ObjKey::model_key` is the one key segment for `model_object_id` and graphify (CR4).
- Re-anchored after `a83bbc43` (text moved, marks kept per the panel): F1-01, F1-02, F1-04,
  F1-06, F1-07, F1-09, F1-10, F1-12, F2-01. 42 entries, every hash re-checked, jq-valid.
- `ci-steps task-affected` green (3834 passed, 8 skipped); `check-goldens` green (996 passed),
  no golden moved; check-diff `[]` before and after the commit.

| test | break | fail | commit |
|---|---|---|---|
| NF1 diff helper status | `run_diff_cmd` exits `max(1)` | 3 diff tests: `alsem diff exit Some(1)` (`/work/bench/83/nf1-broken.log`) | `a83bbc43` |
| CR2 URI `Id` segment | segment back to the object key | `handlers.rs:1344` | `a83bbc43` |
| CR4 graphify unescape (new) | `obj_key_str` without the unescape | `graphify_export.rs:1580` (`@i%22%22foo`) | `a83bbc43` |
| CR5 split object vs collision (new) | panic on every duplicate | `differential.rs:288` | `a83bbc43` |
| CR6 object-run lookup via `SymbolTable` | `SymbolTable`'s `!= 0` guard removed | `combined_graph.rs:1297`, `symbol_table.rs:157` | `a83bbc43` |

Each break reverted; all pass (`/work/bench/83/r9-proof-broken-*.log`).

## Round 10 — final panel round 2 on the replay (2026-10-23)

- Round-9 marks: opus accepted all 12; sol accepted 10 and re-raised CR4 (as NF3) and CR5 (as
  NF2).
- Fixes are commit **`35904042`**:
  - **NF2** (sol, important): `objects_by_id` keeps a duplicate `stableObjectId` row only when it
    equals the previous row; any other duplicate (another name, or the same name with another
    fingerprint) panics naming both rows, so last-wins can no longer hide a divergent row. The
    CR5 test covers the identical, the different-fingerprint and the different-name cases.
  - **NF3** (sol, minor): refuted. Graphify object ids equal `model_object_id` by construction
    (`ObjKey::model_key`); the `a"b` / `a""b` collision is the model's, the spec's
    panel-accepted Limit (D3, discovery D2). No code change; the spec's Limit bullet now says
    graphify's object ids share it.
- Register: 44 entries, every hash re-checked, jq-valid. Reset to `unreviewed`: the 9 spec-doc
  entries whose whole-file anchor changed (R1-02, R1-04, R1-05, R1-07, R1-13, R2-01-R2-04), F1-07
  (its anchor text changed), CR4, CR5, NF2, NF3.
- `differential` 13 passed; check-diff `[]` before and after the commit.

| test | break | fail | commit |
|---|---|---|---|
| NF2 identical vs divergent duplicate | the name-only comparison again | `differential.rs:326` ("same name, other fingerprint on one id must panic"; `/work/bench/83/r10-nf2-broken.log`) | `35904042` |

Break reverted; 13 differential tests pass.

## Final rounds 2-3 and step 11 gates (conductor)

- Final round 2 (head a83bbc43): opus accepted all 12 round-9 ids; sol accepted 10 and re-raised CR4 (NF3) and CR5 (NF2). Reviews: run dir `final-r2-sol.md`, `final-r2-opus.md`.
- Final round 3 (head 35904042): both accepted NF2, NF3, CR4, CR5, F1-07 and the 9 spec-doc entries. sol accepted NF3 "as a documented minor deferral, not a factual refutation", so NF3 is recorded `deferred` (minor). Both: ship. Register: 44 entries, all accepted by both, none open, no blocking entry deferred. final_rounds remaining 2.
- Step 11 gates on 35904042 (supervised, run 20261023-142104-a7c0a4): check-goldens-coverage exit 0 (0.0 s); ci-steps-all exit 0 (382.0 s); check-goldens exit 0 (0.1 s, tree-stamp skip); cdo-gate exit 0 (281.7 s).
- Master moved to 37c9daed (#248, `.claude/commands/issue.md` only) before attest. Rebased cleanly onto it; check-diff `[]`; pushed with `--force-with-lease` (code head fdba751d). Step 11 gates re-run on the rebased tree: check-goldens-coverage exit 0 (0.0 s); ci-steps-all exit 0 (267.6 s); check-goldens exit 0 (0.1 s); cdo-gate exit 0 (162.6 s).
