# Issue #78 ledger

## Claim

```json
{"attempt":1,"body_hash":"17beb9a55c8f1fddf411b4d92ebd070a0ef56659f6cb046745baca2a282df475","branch":"issue/78-agent-discovery-l5-digest-docum-a1","issue":78,"run_id":"20261023-084938-933303","session":"claude-code-session:e2bf58b3-5d1b-467f-8c3b-1c59a8d42f0c","title":"[agent-discovery][l5 / digest (document projection)] Every digest entry's routine.objectDisplay is the empty string: the identity tab","worktree":"/work/al-sem-issue-78-a1"}
```

Issue body hash: `17beb9a55c8f1fddf411b4d92ebd070a0ef56659f6cb046745baca2a282df475`.

Reviewer roster: the operator told us (2026-10-15) to use `gpt-6.1-sol` and `claude-opus-5.5` instead of `gpt-6-astra` and `gemini-3.8-flash`. Marks are recorded as `reviews.sol` and `reviews.opus`. We attest with `--substitute astra=sol --substitute flash=opus`.

## Classification

**bounded.** There is one root cause in one module family (`src/engine/l5`). The fix is one shared index plus three call sites. The design is known, and no new subsystem is needed.

## Design

**Root cause.** `derive_identity_table` (`src/engine/l5/snapshot.rs:1141`) stores the bare routine name (`r.name`) as a routine's display name. `run_digest_query_full_from_entries` (`digest_cli.rs:879`) and `build_block` (`fingerprint_query.rs:310`) both call a private `split_object_routine`, which expects an `Object::Routine` string. No display name has that shape, so the object part is always `""`. `prove` copies the digest entry's `object_display` (`prove.rs:1006`), so it is empty there too. This is one defect that shows up in three documents. All 45 digest, 45 fingerprint and 19 prove `objectDisplay` values in the goldens are `""`.

**Fix (revised after spec round 1).** Take the owner from the Model, the same source `query effects` joins. Do not take it from the snapshot's identity table. Round 1 showed the identity table is not an authoritative object-name map, for two reasons:
- Every unnumbered object (Interface, ControlAddIn) in an app shares the object id `<guid>/<Kind>/0` (`workspace.rs:778`, `:897-899`), so the table keeps one name per kind per app.
- A tableextension stub's table id collides with a real table of the same number, and `ws.tables` is inserted after `ws.objects` (`snapshot.rs:1147-1152`, `workspace.rs:184-211`), so a stub can overwrite the real table's name.

The rule and its index live ONCE, in `src/engine/l4/effect_query_cli.rs`, beside `RoutineJoin`:
- `pub fn object_display(object_type: &str, object_name: &str) -> String` is `format!("{object_type} {object_name}").trim().to_string()`. `RoutineJoin::of` calls it in place of its inline `format!`. So `query effects` and the new index cannot drift apart. That includes the missing-object case, which prints the bare type.
- `pub fn object_display_by_stable_routine_id(resolved: &Model) -> HashMap<String, String>` takes `&Model`, because `RoutineJoin::build` needs it (`effect_query_cli.rs:468`). It fills only PRIMARY-app rows: in cross-app mode `workspace.routines` also holds every source-bearing dependency routine, and only workspace roots are ever looked up. The filter is the module's own rule, `is_primary_routine` (`effect_query_cli.rs:436`), and the same app-guid test for objects. It fills from two sources:
  - Every primary `ws.routines` row maps `r.stable_routine_id` to `RoutineJoin::of(r).object_display`, the exact `query effects` value.
  - Every primary `ws.objects[o].declaration_only_routines[d]` row maps `d.stable_routine_id` to `object_display(&o.object_type, &o.name)`. It is read from the declaring object itself, so two interfaces never share an owner. Their stable routine ids already fold in the object name (`workspace.rs:897-924`). The two sources are disjoint: the ordinary routine loop skips Interface and ControlAddIn.
  - For a stable id shared by several rows (the `#if` arms of one routine), the first row wins. The rows share the stable-object prefix and so the same internal object id, which means every row gives the same value. This also matches `query effects`' stable-id selector, which takes the first matching routine (`effect_query_cli.rs:391-396`).
  - **Inherited limit (not fixed here).** `RoutineJoin` keys object names by internal object id, last row wins (`effect_query_cli.rs:469-479`). Two model objects that share an internal id both get the surviving name. That happens with duplicate MODEL object ids, which arise three ways:
    - mutually exclusive `#if`/`#else` declarations of one type and number with different names. This is valid AL: the lowerer keeps both arms (`crates/al-syntax/src/lower/mod.rs:68-124`), and the physical index and the Model keep both rows (`src/program/physical.rs:76-109`, `workspace.rs:730-750`);
    - an object whose number is missing, which falls back to 0 (`workspace.rs:777`);
    - a duplicate type and number in one app.

    That is true in `query effects` today, and the new index inherits it for ordinary routines. Declaration-only rows avoid it, because they read the declaring object directly. #78 is bounded to parity with `query effects`. The limit is recorded under Not delivered and filed as a discovery.

Consumers:
- `run_digest_query_full_from_entries` (`digest_cli.rs:779`) takes the index as one more argument. Both entry kinds (`:850` declaration-only, `:1005` normal) set `object_display` from it, or to `""` when the id is absent. The absent case is a fail-closed placeholder that never invents a name. Callers: `digest_cli.rs:1632` and `prove.rs:968`, which both hold the Model. Prove needs no other change, because it copies `entry.object_display`.
- `fingerprint_query` (`fingerprint_query.rs:649`) takes the index and `build_block` reads it. The caller `fingerprint_cli.rs` holds the Model (`:350`).
- Both private copies of `split_object_routine` are deleted (`digest_cli.rs:633`, `fingerprint_query.rs:244`). The routine display becomes the identity name verbatim, so a routine name containing `::` is no longer split. `parse_object_id_from_routine` stays; it serves the block's `objectId`, not the display.
- The snapshot, its identity table, selectors (`selector_index.rs`), `owningRoutineDisplay` and the CBOR/shard encodings are all untouched.

**Scope note.** The issue names digest. Fingerprint and prove carry the same empty field from the same root cause, and the shared index fixes them as well. There are three call sites (digest normal, digest declaration-only, fingerprint `build_block`); prove inherits the fix. Expected golden movement:
- digest JSON `routine.objectDisplay`, and the digest human header, which already prints `object::routine` only when the object is set (`digest_cli.rs:1447`).
- prove JSON `objectDisplay` and its human header (`prove.rs:746`).
- fingerprint JSON `objectDisplay` and its human header, which always prints `{object}::{routine}` (`fingerprint_query.rs:1523`), so it shows `::Name` today.
- Fingerprint CBOR, `.cbor.gz` and shard goldens must stay byte-identical. The triager treats any move there as an error.

## Assumption probes

| probe | result | evidence |
|---|---|---|
| digest `objectDisplay` is always `""` | confirmed (45 now; the issue said 41) | `grep -rhoE '"objectDisplay": "[^"]*"' tests/cli-b-goldens/digest` |
| the identity table's routine display is the bare name | confirmed | `snapshot.rs:1153-1155` |
| the identity table holds every workspace object's stable id to its name | confirmed | `snapshot.rs:1147-1149` |
| stable routine id = `<stableObjectId>#<hex>` | confirmed | `ids.rs:315-323` |
| query effects' rule is `"{object_type} {object_name}".trim()` | confirmed | `effect_query_cli.rs:494-503` |
| the same defect is in fingerprint and prove | confirmed (45 and 19 `""`) | `fingerprint_query.rs:310`, `prove.rs:1006` |
| declaration-only routines' objects are in `ws.objects` | confirmed | `digest_cli.rs:322-335` (`objects.flat_map(declaration_only_routines)`) |

## Base/Head SHAs

| round | B (origin/master) | H |
|---|---|---|
| gates 1 | `662f5137a54253496dedb2c81277f639492f2ffc` | `ffd3c14c9fd081b712e7e0cb92a1c32f197c8db1` |
| gates 2 | same | `ceb18ca0` |
| gates 3 / freeze | `662f5137a54253496dedb2c81277f639492f2ffc` | `55e53b9b0fd33f1ecc3297d8605ed3ad6514b84a` |
| replay (run `20261023-084938-933303`) | `84ba2d4b` | `ae89850d` |

No rebase was needed: `origin/master` never moved from the claim base. `check-diff --base B --head H` gave no reasons.

## Timeline

- Worktree created at master `662f5137`.
- Classified as bounded; design written.
- Spec panel (sol + opus, operator roster), 5 rounds (spec_rounds 5/5; pi_calls 10):
  - r1: both REJECTED the identity-table design (Interface `<guid>/<Kind>/0` collision, tableextension stub overwrite) and it was redesigned to a Model-based owner;
  - r2: sol re-raised S3 (no prove test), so A4 was added; minor N1-N5;
  - r3: sol accepted all, opus minor P1/P2;
  - r4: sol re-raised P1 (`#if` duplicate object ids are valid AL), opus Q1;
  - r5: both accepted all 16 entries, no new finding from sol, and one opus formatting nit (R1, fixed, marks carried to final-panel round 1).
  Converged: no `open`, every blocking entry `fixed`.
- Acceptance tests (opus subagent, subagents 1). The tests are RED for the stated reason.
  - Tests: A1 `cli_object_display::digest_entries_name_their_owning_object`, A2 `…::fingerprint_block_names_its_owning_object`, A4 `…::prove_names_its_owning_object`, and A3, which extends `cli_digest_changed_spans::declaration_only_procedures_are_roots`.
  - Every failure is `""` vs the literal owner (`Table OD Item`, `TableExtension OD Item Ext`, `Codeunit OD Cu`, `Interface IFoo`/`IBar`).
  - Preconditions that pass: the identity table holds `OD Item Ext` at `<g>:Table:50100`, which is the stub overwrite; query effects gives the literal for every non-declaration-only root; the fingerprint OnInsert block is `trigger-table`; prove exits 0.
  - Interface roots have no query-effects counterpart (`unmatched`, exit 2), so their literal follows the design rule.
- Plan: `docs/superpowers/plans/2026-10-16-issue-78-digest-object-display.md`, 2 tasks (plan_tasks 2).
- T1 implement (opus): 4/4 acceptance tests green; lib 2031 passed; clippy 0. Expected failures: 11 cli-b golden tests.
  - Deviation in form: `run_digest_query_full_from_entries` takes `&ChangedRootsResult` instead of `spans` + `touched`. Clippy's `too_many_arguments` fails at 8 parameters, and `#[allow(` is banned.
- T1 review (sonnet): 0 critical, 1 important, 4 minor.
  - Important: the primary-app test was written twice. Fixed with `is_primary_app_guid`.
  - Minor fixed: a unit test for the primary filter and first-wins (U1); the test-module doc; the CHANGELOG headers.
  - Minor deferred: the fingerprint header could guard an empty owner, like digest and prove do. It is outside the ratified design, and a workspace root always has an owner.
- T1 review-fix (sonnet): lib 2032 passed; clippy 0. subagents 4, task_attempts T1 1.
- T2: golden triage (golden-diff-triager; receipt `.agent/golden-triage.md`): 123 files, 211 lines, 0 unexplained. Then `check-goldens-regen` exit 0. Commit `ffd3c14c` (task 1 + task 2 together, because the hook rejects red goldens); the pre-commit goldens passed and `check-diff` gave no reasons.
- Gates round 1 at `ffd3c14c`: all 0 (see Gate results).
- Final panel round 1 (final_rounds 1; pi_calls 12):
  - `code-review` high (built-in skill) raised 9 findings.
  - sol and opus accepted all 17 register entries. New minor findings: sol F1/F2 (CHANGELOG), opus 1 (CHANGELOG) and opus 2 (digest doc).
  - Fixes:
    - One shared `owner_qualified` header rule for digest, fingerprint and prove. The fingerprint header no longer prints `::Name` for an unknown owner (code-review 1, and the T1 review's deferred item).
    - The owner-index doc names the inherited limit (code-review 4).
    - The digest doc explains the new parameter (opus 2).
    - CHANGELOG reworded (sol F1/F2, opus 1).
    - Test A5 and unit test U2 added (code-review 9).
  - The other code-review findings: 2, 3 and 5 are deferred as discoveries (pre-existing); 6, 7 and 8 are refuted (see the register).
  - Commit `ceb18ca0`; hook goldens OK; `check-diff` gave no reasons. Gates round 2 at `ceb18ca0`: all 0, supervised, porcelain clean. Logs: `logs/ci-steps-all/20261016T102830-d9f413ba.log`, `logs/check-goldens-coverage/20261016T103930-64c5c701.log`, `logs/check-goldens/20261016T103930-571e1618.log`, `logs/cdo-gate/20261016T104027-883353fd.log`.
- Final panel round 2 (final_rounds 2; pi_calls 14): sol accepted all 28 entries with no new findings, including deferring C3. Opus accepted all 28 and raised two new minor items, G1 (the A5 prove assert was loose) and G2 (CHANGELOG wrap). Both fixed; break-c re-proven against the tighter assert.
- `origin/master` is still `662f5137` (fetched after round 2), so no rebase is needed.
- Commit `55e53b9b` (G1/G2). Final panel round 3 (final_rounds 3; pi_calls 16): both reviewers accepted all 30 entries; no new findings. CONVERGED. (P1's mark was corrected from sol's round-4 `re-raised` to `accepted`: sol accepted it in spec round 5 and in every final round.)
- Helper reports (acceptance-red, impl-t1, review-t1, fix-t1, triage-t2) moved to `.agent/runs/20261016-090116-97024b/issue-reports/`; only the ledger and the register are committed.
- 2026-10-23, run `20261023-084938-933303` (attempt 1, worktree `/work/al-sem-issue-78-a1`): the old attempt stopped only on red CI after an external crate yank. Its three code commits (pushed as `39c238e7`, `096cfb84`, `62d2b625`; the `ffd3c14c`/`ceb18ca0`/`55e53b9b` above are their pre-push SHAs) were cherry-picked onto `70fc2e9b`, then rebased cleanly onto `84ba2d4b` (#156) as `5dac4bc7`, `e19059f5`, `4c524f21`. Conflicts: CHANGELOG (the entry moved to `[Unreleased]` Fixed), imports in `digest_cli.rs`/`fingerprint_cli.rs`/`prove.rs`, `tests/cli/main.rs`, and `effect_query_cli.rs`, where #220's role rule replaced `is_primary_app_guid` (`routine_role`/`object_role == Role::Primary`; same rule) and the U1 test rows gained master's new fields; `cli_object_display.rs` now reads the workspace through `InputSet::read`. `ae89850d` drops two needless borrows clippy-dev flagged (master's `resolved` is already a reference). `check-goldens --regen` moved nothing further; against master the replay moves the same 123 files and 211 lines (108 `objectDisplay`, 62 digest/prove headers, 41 fingerprint headers, 0 other). On `84ba2d4b`: `ci-steps task` green (3798 passed), `check-goldens` 972 passed; `check-diff` gave no reasons. Register entries re-anchored to the replayed code (`line`/`line_end`/`anchor_hash`); marks kept only where the anchored text is byte-identical to the old head `62d2b625`.
- Final panel round 1 on the replay (`ae89850d`): sol and opus accepted S4 and N5, stood by the 27 carried marks, and re-raised C6 (its evidence cited a whole-model S4 pass that #174 removed). New: sol F1 (filed as F3, because F1 is taken), opus G3, code-review CR1-CR7. Fix commit `1c95cc8b` (CR1-CR6, G3): an `<owner>::<routine>` selector narrows to that owner (digest, fingerprint, prove); ambiguity candidates print the owner; `OwnerIndex` replaces the String-per-routine map and is built once per digest/prove run; `query` headers use `RoutineRef::qualified`; U1 is rebuilt on `test_support` rows with a possible precondition. This also fixes C2 and part of C3 (were deferred discoveries). CR7 deferred (measured cheap). No golden moved (`check-goldens` 974 passed). The register moved to the 12-field shape (`anchor` = `path:L1-L2`); marks were reset on every entry whose anchored text changed in `1c95cc8b` (diffed against `ae89850d`), and on C2, C3 and C6, whose disposition or evidence changed.

## Acceptance matrix

| Acceptance item | Proof |
|---|---|
| `objectDisplay` is `'<ObjectType> <ObjectName>'` as in query effects | **A1** (new `tests/cli` test on a new fixture that holds a codeunit, a Table 50100 and a TableExtension 50100 with the same number, each with a procedure, and the Table also with an `OnInsert` trigger so that it is root-classified. The files are named so the TableExtension sorts AFTER the Table (`a_Table.al`, `b_TableExt.al`). The test asserts the PRECONDITION that `snap.identities` holds the extension's name at `<g>:Table:50100`; otherwise the identity-table substitution could not be caught. The fixture also has a table field `OnValidate` member trigger; and two Interfaces `IFoo`/`IBar`, each with a method). For every digest root it asserts `routine.objectDisplay` equals a literal expected owner (`Table X`, `TableExtension Y`, `Interface IFoo`, `Interface IBar`, …) and is non-empty. For each non-declaration-only root it also asserts equality with `alsem query effects`' `objectDisplay` for the same stable id. **A2**: one fingerprint assertion on the same fixture, on the REAL Table's block (its `OnInsert` trigger; the test asserts the precondition that this block exists, i.e. the trigger is root-classified) (the one whose identity-table name the extension stub overwrites, `snapshot.rs:1147-1152`), where `objectDisplay` equals the literal `Table <name>`. **A4**: one `alsem prove` assertion on the same fixture, on the real Table's routine, where the JSON `objectDisplay` equals the same literal (prove has its own caller of the digest query, `prove.rs:968`). The selector is that routine's STABLE ID, which matches exactly; a bare name could be ambiguous between the Table and the TableExtension. The test asserts `exit_code == 0` as a precondition, since the exit-2 path prints `objectDisplay: ""` for an unrelated reason. **A3**: the existing declaration-only test (`tests/cli/cli_digest_changed_spans.rs`, the IFoo/IQux/IEmpty fixture) gains exact `objectDisplay` asserts. Each gets a discrimination proof (revert the lookup, and also substitute the identity-table name, and watch the Table/Interface asserts fail). |
| digest goldens move only in that field (and the human header, which prints `object::routine` when set) | `golden-diff-triager` over the regen diff: every moved digest/prove/fingerprint line is an `objectDisplay` value or a human header line; CBOR/shards unchanged |

## Discrimination proofs

Every break was made with Edit and checked to have applied (the edited line was grepped). It was reverted with Edit and the file touched before re-running.

Mutation (i): the owner lookup returns `""`. Mutation (ii): the owner comes from the snapshot identity table by object stable id, which is the rejected design.

| test | mutation | fail output (decisive) | after revert |
|---|---|---|---|
| A1 `cli_object_display::digest_entries_name_their_owning_object` | (i) | every owner `""` vs the literals | ok |
| A1 | (ii) | `("src/a_Table.al","OnInsert","Table OD Item Ext")`, `("src/d_IFoo.al","FooMethod","Interface IBar")` | ok |
| A2 `…::fingerprint_block_names_its_owning_object` | (i) | `left: ""  right: "Table OD Item"` | ok |
| A2 | (ii) | `left: "Table OD Item Ext"  right: "Table OD Item"` | ok |
| A3 `cli_digest_changed_spans::declaration_only_procedures_are_roots` | (i) | `left: ""  right: "Interface IFoo"` | ok |
| A3 | (ii) | `left: "Interface IQux"  right: "Interface IFoo"` | ok |
| A4 `…::prove_names_its_owning_object` | (i) | `left: ""  right: "Table OD Item"` | ok |
| A4 | (ii) | `left: "Table OD Item Ext"  right: "Table OD Item"` | ok |
| U1 `effect_query_cli::tests::object_display_map_filters_to_primary_and_first_row_wins` | routine filter removed | `dependency routine leaked: {… "s-dep": "Codeunit DepCu" …}` | ok |
| U1 | object filter removed | `dependency decl leaked: {"decl-dep": "Interface DepIface" …}` | ok |
| U1 | `or_insert_with` changed to `insert` | `left: Some("Codeunit Beta")  right: Some("Codeunit Alpha")` | ok |

| U2 `effect_query_cli::tests::owner_qualified_omits_an_unknown_owner` | empty-owner branch made `if false` | `left: "::Run"  right: "Run"` | ok |
| A5 `cli_object_display::human_headers_name_the_owning_object` | digest header prints the routine only (break-b) | panics at `cli_object_display.rs:336`; the header reads `--- OnInsert ---` | ok |
| A5 | prove header prints the routine only (break-c) | panics at `:366`; `--- OnInsert ---` (re-run as break-c2 against the tightened line-equality assert, G1: panics at `:366`, `proof-c2.log`) | ok |
| A5 | fingerprint header prints the routine only (break-d) | panics at `:351`; `OnInsert  [trigger-table]` | ok |

U2 and A5 were added in the final-panel round-1 fix. Their proofs were made on that tree; logs are `.agent/runs/20261016-090116-97024b/proof-{a,b,c,d,revert}.log`. Stated limit: no CLI test exercises an EMPTY owner through the fingerprint renderer, because no workspace root lacks an owner. That branch is pinned by U2 on the shared `owner_qualified` alone.

All earlier proofs were made on the uncommitted Task 1 tree, which is committed with the goldens as one commit (the pre-commit hook rejects a red golden family).

Final panel round 1 on the replay (fix commit `1c95cc8b`; proofs made on that tree before the commit; logs `/work/bench/78/proof*.log`). Red first: A6 failed at `cli_object_display.rs:430` (`left: 2 right: 0`, the qualified digest selector was ambiguous); A7 at `:474` (`left: ["Persist","Persist"]`). Each break was made with Edit, grepped to confirm it applied, and reverted with Edit; after the reverts the 5 lib tests and the 6 `cli_object_display` tests pass (`proof-revert-{lib,cli}.log`). The U1 row in the table above (`or_insert_with` -> `insert`) no longer applies: the test was rebuilt (CR6).

| test | mutation (file:line, what) | fail output | pass output | commit |
|---|---|---|---|---|
| A6 `cli_object_display::owner_qualified_selector_narrows_to_its_owner` + U3 `selector_index::tests::narrow_to_owner_keeps_the_named_owner_or_everything` | `selector_index.rs` `narrow_to_owner`: form check made `!= "BREAK-object-qualified"` (never narrows) | A6 panics at `:430`, `left: 2 right: 0`; U3 `left: ["a","b"] right: ["a"]` | ok | `1c95cc8b` |
| A6 | `fingerprint_query.rs:671`: `narrow_to_owner(sel, "BREAK", …)` (fingerprint call site only) | panics at `:449`, fingerprint exit `left: 2 right: 0` | ok | `1c95cc8b` |
| U3 | `selector_index.rs` fallback: empty narrowing returns `narrowed` | panics at `selector_index.rs:250`, `left: [] right: ["a","b"]` | ok | `1c95cc8b` |
| A7 `cli_object_display::ambiguous_selector_candidates_name_their_owner` | `digest_cli.rs:421`: candidate = `display.to_string()` | panics at `:474`, `left: ["Persist","Persist"]` | ok | `1c95cc8b` |
| A7 | `fingerprint_query.rs:685`: candidate = `display.to_string()` | panics at `:491`; stderr `  - Persist  (…:Codeunit:50101#…)` | ok | `1c95cc8b` |
| U1 `effect_query_cli::tests::owner_index_filters_to_primary_and_follows_routine_join` | `OwnerIndex::build` routine filter `\|\| true` | panics at `:1360`, `left: "Codeunit DepCu" right: ""` | ok | `1c95cc8b` |
| U1 | object filter `\|\| o.id.contains('/')` | panics at `:1361`, `left: "Interface DepIface" right: ""` | ok | `1c95cc8b` |
| U1 | name map built over `.iter().rev()` (first wins) | panics at `:1368`, `left: "Codeunit Old" right: "Codeunit New"` | ok | `1c95cc8b` |

CR5 (`RoutineRef::qualified` in `query`) has no new test: a `RoutineRef` owner is never empty (its object type is always set), so the output is byte-identical, and the `query` goldens and differentials pass unchanged.

## Findings register summary

Old attempt: 30 entries, every one accepted by both sol and opus.
- 23 `fixed`, including every critical and important entry (S1, S2, S3).
- 3 `refuted`: C6 (perf), C7 (the bare-type fallback is the ratified parity rule) and C8 (the `ChangedRootsResult` grouping).
- 4 `deferred`, all minor and pre-existing, and all filed as discoveries: N1, C2, C3, C5.

After final panel round 1 on the replay: 39 entries. C2 and C3 are now `fixed`. F3, G3 and CR1-CR6 are new and `fixed`; CR7 is new and `deferred` (minor). Still `deferred`: N1, C5, CR7. The marks to collect are listed in the register (`unreviewed`).

## Gate results

Round 1, at H = `ffd3c14c9fd081b712e7e0cb92a1c32f197c8db1`. Every gate ran with `supervised: true`.

| gate | exit | log | tests |
|---|---|---|---|
| ci-steps-all | 0 | `.agent/runs/20261016-090116-97024b/logs/ci-steps-all/20261016T095911-92424947.log` | 3275 passed, 0 failed |
| check-goldens-coverage | 0 | `…/logs/check-goldens-coverage/20261016T101033-ccfe0bd3.log` | coverage OK |
| check-goldens | 0 | `…/logs/check-goldens/20261016T101033-887af21d.log` | 787 passed, 0 failed |
| cdo-gate | 0 | `…/logs/cdo-gate/20261016T101142-3173d3bd.log` | 2250 passed, 0 failed; ratchets green |

- After the gates, `git status --porcelain -- . ':!.agent'` was empty.
- Toolchain: rustc 1.96.0. Grammar: tree-sitter-al `8682cb5f`. CDO_WS identity hash (sha256 of path@HEAD, 16 hex): `f1d20e6aac6f8b6e` (pinned baseline at `bc3ccb18`).
- Rounds 2 and 3 re-ran every gate after the panel fixes:
  - round 2 at `ceb18ca0`: all 0;
  - round 3 at H `55e53b9b`: all 0, supervised, porcelain clean. ci-steps-all 3277 passed / 0 failed (`logs/ci-steps-all/20261016T104455-03d105c4.log`); check-goldens-coverage 0 (`logs/check-goldens-coverage/20261016T105029-ea603480.log`); check-goldens 788 / 0 (`logs/check-goldens/20261016T105029-c59da36c.log`); cdo-gate 2251 / 0, ratchets green (`logs/cdo-gate/20261016T105125-bfa2046e.log`).
- Replay, final panel round 1 fix, at `1c95cc8b` (base `84ba2d4b`): `ci-steps task` exit 0, 3801 passed (`/work/bench/78/ci-steps-task-r1.log`); `ci-steps task-affected` exit 0; `check-goldens` 974 passed with nothing moved (`/work/bench/78/check-goldens-r1.log`); `check-diff` gave no reasons before and after the commit. The branch gates (`ci-steps all`, `check-goldens --verify-coverage`, `cdo-gate`) are not run yet.
- Golden regen: `check-goldens-regen` exit 0, `…/logs/check-goldens-regen/20261016T095412-0752ba1b.log`. It moved 123 files (digest 41, fingerprint 46, prove 36), and all of them were triaged before the regen with 0 unexplained.

## Discoveries

```json
[
  {"subsystem": "l5 / selector index (object-qualified form, unmatched owner)", "locator": "src/engine/l5/selector_index.rs narrow_to_owner (fallback when no match has the named owner)", "symptom": "Since #78 an '<owner>::<routine>' selector narrows form 5's matches to that owner. When NO matched routine has the named owner (a typo, a bare object name, an object that does not declare the routine), it falls back to form 5's matches, so 'Codeunit C::Persist' with Persist only in Codeunit B still answers for B's routine, and with Persist in A and B it is ambiguous rather than unresolved.", "kind": "bug", "origin_issue": 78, "reproducer": "A workspace where only Codeunit B declares procedure Persist: alsem prove <ws> 'Codeunit A::Persist' may-commit -> answers for Codeunit B::Persist without a warning.", "pre_existing": true, "capability": "an object-qualified selector whose object part names no owner of a matching routine is unresolved (or warns), and a bare object name is accepted as the object part", "acceptance": "a cli test: '<other owner>::<routine>' with no such routine is selector-unmatched, not another object's routine; '<ObjectName>::<routine>' selects that object's routine."},
  {"subsystem": "program model / object names (quoted identifiers)", "locator": "src/program/model/workspace.rs:778 (ModelObject.name = IR name, escapes kept) vs :907 (unescape_al_identifier for the logical name); consumers src/engine/l4/effect_query_cli.rs RoutineJoin::of", "symptom": "An object whose quoted name contains an escaped quote (\"My \"\"Q\"\" Iface\") is displayed with the doubled quotes ('Interface My \"\"Q\"\" Iface') by query effects, digest, fingerprint and prove, while its stable id folds in the unescaped logical name.", "kind": "bug", "origin_issue": 78, "reproducer": "By code inspection (not run): interface \"My \"\"Q\"\" Iface\" { procedure P(); } then alsem digest <ws> --file <that file> --format json -> routine.objectDisplay 'Interface My \"\"Q\"\" Iface'.", "pre_existing": true, "capability": "object displays show the logical (unescaped) AL name", "acceptance": "a fixture with an escaped-quote object name shows the unescaped name in query effects and digest objectDisplay."},
  {"subsystem": "l4 / RoutineJoin (object name by internal id)", "locator": "src/engine/l4/effect_query_cli.rs:469-479 (object_name_by_id, last insert wins)", "symptom": "Two model objects that share one internal object id get the surviving object's name for every routine of either: mutually exclusive #if/#else object declarations of one type and number with different names (valid AL; both arms are kept by the lowerer and the Model), a missing object number (falls back to 0), or a duplicate type and number. query effects, and since #78 digest/fingerprint/prove, show that name.", "kind": "bug", "origin_issue": 78, "reproducer": "By code inspection (not run): #if A codeunit 50100 \"Foo\" { procedure P() ... } #else codeunit 50100 \"Bar\" { procedure Q() ... } #endif -> query effects for P prints objectDisplay 'Codeunit Bar'.", "pre_existing": true, "capability": "each routine's owner display names the object declaration that contains it", "acceptance": "a fixture with #if-split same-id objects: each routine's objectDisplay names its own declaring object in query effects and digest."}
]
```

## Not delivered

- The owner of an ordinary routine whose object shares an internal id with another object of the same kind. That is a duplicate model object id, which arises from mutually exclusive `#if` object declarations of one type and number, from a missing object number (it falls back to 0), or from a duplicate type and number. This is a pre-existing `query effects` limitation (`RoutineJoin`, `effect_query_cli.rs:469-479`), inherited by design for parity (spec round 2, N1). Filed as a discovery.
- An `<owner>::<routine>` selector whose object part matches no matched routine's owner falls back to form 5's matches (CR1's stated limit). It is not unresolved, and a bare object name does not narrow. Filed as a discovery. (C2, candidates without owners, and the rest of C3 are fixed in `1c95cc8b`.)
- `cli_object_display`'s per-test process spawns (CR7). Deferred; measured cheap (see the register).
- Owner names keep a quoted AL name's inner `""` escapes (C5). Pre-existing in `query effects`; filed as a discovery.

## Attempt-1 replay: final panel and step 11 gates (conductor record, appended so no anchor moves)

- Final panel on the replay (reviewers sol in slot astra, opus in slot flash; `--substitute astra=sol --substitute flash=opus`): r1 (final_rounds 1/5) — built-in `code-review` at high (CR1-CR7) + sol + opus; both stood by the 27 carried marks, accepted S4/N5, re-raised C6 (stale evidence; sol's F1 filed as F3, opus G3) → fixes `1c95cc8b`. r2 (2/5): all 18 marked ids accepted by both, no new findings, both **ship**. Register: 39 entries, 0 open, all accepted by both, no blocking entry deferred (CR7 deferred, minor). Review files `.agent/runs/20261023-084938-933303/final-r{1,2}-{sol,opus}.md`, `final-r1-codereview.md`.
- **Step 11 full gate set** on H = `1c95cc8b53f9c68841ec7df7d6f16639ccd9ae0c`, B = `84ba2d4b` (origin/master unmoved since the replay's rebase), all supervised, queued 0 s, from the worktree, `CDO_WS` exported from `.agent/config.json`, `cdo-gate` with no argument:
  - `check-goldens-coverage`: exit 0. Log `.agent/runs/20261023-084938-933303/logs/check-goldens-coverage/20261023T094155-4e926d28.log`.
  - `ci-steps-all`: exit 0, 322.6 s. Log `…/logs/ci-steps-all/20261023T094155-46175979.log`.
  - `check-goldens`: exit 0, 0.1 s (tree-stamp skip after `ci-steps all` on the same tree). Log `…/logs/check-goldens/20261023T094718-32d19735.log`.
  - `cdo-gate`: exit 0, 364.2 s. Log `…/logs/cdo-gate/20261023T094718-253df82e.log`.
