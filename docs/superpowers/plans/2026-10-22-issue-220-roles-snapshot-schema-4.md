# #220 (epic #160 step 8): roles, snapshot schema 4, preflight — plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development
> (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps
> use checkbox (`- [ ]`) syntax for tracking.

**Goal:** one role rule (`Role`) for every row; live analysis gaps filtered by reachability when
serialized; snapshot schema 4 (`analysisUniverse`, `apps[].role`, gap `role`, key classes with a
typed reference closure); a diff preflight that refuses schema 3, bad universes/roles and mixed
universes, reads identity by role, and exits 1 when it is fatal.

**Architecture** (spec §4):
- `src/program/model/role.rs` (new) holds the rule; every site of spec §3.1 calls it.
- `src/engine/l5/snapshot.rs` derives gaps with the live rule; `snapshot_full.rs` carries the
  universe (`FullSnapshotOptions.universe`), the class tables, the typed-row filter, the closure
  check and the shard closure.
- `src/engine/gate/snapshot_deserialize.rs` and `gate/diff/{preflight,mod,cli}.rs` own the
  load refusals, the universe check and the exit code.
- No command changes universe: every production caller passes `SnapshotUniverse::SingleApp`.

**Tech stack:** Rust; `scripts/ci-steps`, `scripts/check-goldens`; the stdlib CBOR decoder
`/work/bench/160-8/cbordec.py`; a new `/work/bench/160-8/allowlist.py` (outside the repo).

**Spec:** `docs/superpowers/specs/2026-10-22-issue-220-roles-snapshot-schema-4-design.md`
(converged at `28aede25`, 49 register entries; "spec §n" below). Ledger:
`.agent/issue-220/ledger.md` (probes P1-P12, `## Acceptance tests (red)`).

Seven tasks, one commit each, in this order. Only **Task 2** moves goldens (the schema bump,
the 12 hand-edited diff inputs and the regen all land in it). Every other task must leave
`scripts/check-goldens` green with no golden file changed; a moved golden outside Task 2 is a
defect (spec F5, F26), never a regen.

| Task | Title | Small? | Red tests it turns green (and commits) |
|---|---|---|---|
| 1 | `Role`, the one rule, at every site | no | T1, T1c, T2 |
| 2 | Schema 4 shape + the golden move | no | T7, T11, T12, `wrong_schema_version_errors` |
| 3 | Live gaps, reach over the cone | small | T3, T4, T4c, T5 |
| 4 | Key classes, typed filter, closure | **large** | T9a, T9b, T9d, T9e, T9f, T9g, T9-closure ×4, T9-keys composed, T10 |
| 5 | Shards by role, shard closure | small | T8, T9-keys shard |
| 6 | Preflight: load refusals, universe, role identity, exit 1 | small | T6, T6b, T7b, T8b |
| 7 | CHANGELOG, `allowlist.py`, ledger | small | — (Part 0 check) |

## Global Constraints

- **Editing.** Edit/Write tools only. No `sed -i`, `perl -pi`, heredoc, `python` or script
  rewrites of any repo file, discrimination breaks included (break with Edit, run, revert with
  Edit; re-read the edited line before running the break to confirm it applied). Generated
  files (`check-goldens --regen`, `rustfmt`) are the only exception.
- **Formatting.** `rustfmt <file>` per touched `.rs` file; never `cargo fmt`.
- **No silencing.** No `#[allow(` and no `#[ignore` in the branch. Dead code is deleted.
- **Protected paths, never edited:** `.github/`, `scripts/`, `.claude/`, `CLAUDE.md`,
  `.gitignore`, `Cargo.toml` version fields, `tree-sitter-al`, `.agent/` (except the ledger's
  `## Discrimination proofs`, `## Gate results`, `## Discoveries` rows and Timeline lines).
- **Environment.** `export TREE_SITTER_AL_PATH=/work/alsem/tree-sitter-al` in every shell. CDO
  only as a per-command prefix `CDO_WS=$(jq -er .cdo_ws /work/alsem/.agent/config.json) <cmd>`;
  never write the path into a file, log name or ledger row. Never quote a `cdo`-profile timing.
- **Per-task gate, before the commit.**
  1. `scripts/ci-steps task-affected > /work/bench/160-8/gate-T<n>.log 2>&1; echo $?`, then
     `grep -nE 'FAILED|^error|test result' /work/bench/160-8/gate-T<n>.log` (never `| tail`).
  2. `scripts/check-goldens > /work/bench/160-8/goldens-T<n>.log 2>&1; echo $?` = 0.
  3. `git status --short tests/*goldens* tests/goldens` is empty except in Task 2.
  4. The pre-commit hook passes unmodified (never `--no-verify`).
- **After each commit:** `python scripts/agentflow check-diff --base master --head HEAD
  --issue 220 --cwd /work/al-sem-issue-220-a1` prints `reasons: []`; anything else is fixed in a
  new commit, never amended.
- **Staging.** `git add <paths>` of the task's files only; never `git add -A`. Commit messages
  name `#220` and end with `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`.
- **Discrimination proof** for every test a task turns green: the break the spec (§5) states,
  the failing line it produced, the revert, the pass. One row per break in the ledger's
  `## Discrimination proofs` table (test, mutation, fail output, pass output, commit).
- **Test commands.** Lib: `cargo test -p al-sem --lib <name>`. cli member:
  `cargo test --test cli cli_snapshot_schema4::<name>`. Never pipe a test run through `tail`.

## The red tests: where they come from

The acceptance tests and their compile stubs are uncommitted in the worktree (ledger
`## Acceptance tests (red)`). Each test is committed with the task that makes it green, so the
tree at each commit must hold only that task's tests and earlier ones.

### Task 0 (setup, no commit)

- [ ] Save reference copies, outside the repo:
  `mkdir -p /work/bench/160-8/red && git diff > /work/bench/160-8/red/red.patch` and
  `cp --parents` every modified and untracked path of `git status --short` into
  `/work/bench/160-8/red/tree/` (whole files, at their repo-relative paths).
- [ ] Park the edits: `git stash push -u -m "220 red acceptance tests"` (kept as a backup; never
  dropped until Task 7 is committed). `git status --short` is empty.
- [ ] From here on, each task brings in its tests and stubs by reading
  `/work/bench/160-8/red/tree/<path>` and adding the needed text with Edit/Write. A stub body
  `unimplemented!("#220")` is never committed: the task replaces it with the real body. Stub
  signatures are the implementer's to change, together with the tests that call them.

Deviations from the spec's letter, carried from the ledger (do not "fix" them):
- **T9d:** the spec's precondition "in memory, `Outer`'s `callsiteIndex` row exists" is false
  until step 6 (`derive_callsite_evidence`, `snapshot.rs:1974`, keeps only call sites a cut row
  references; `Outer → Inner` is cone-only). The test asserts the base truth (no such row in
  memory) with a message to restore the spec's precondition at step 6, so its "NOT `Outer`'s
  call-site row" arm is vacuous until then. Its proofs are breaks 1, 2 and 4 only.
- **T2** is green before Task 1 (it pins what `cross_app_scope` already does). It becomes a use
  test once the registry calls the rule; its break exists only after conversion.
- **T9e** carries an extra precondition, `analysisUniverse == "cross-app"`, so it cannot pass
  vacuously on two unfiltered trees.
- **T10**'s "no `REFERENCES` field names `Unused`" excludes defining fields (`stableId` names it).
- **T9-closure** is four tests (ledger deviation 4): two composed in the cli member
  (`closure_fields_are_classified_and_closed`, `closure_reports_each_dangling_domain`), two on
  hand-built typed rows in `snapshot_full.rs` (`closure_nested_references_keep_their_rows`,
  `closure_literal_keeps_nothing`, as F30 allows).

## Review Focus

- **An upper-case manifest guid** (primary or dependency) must give the same role, `apps[]`
  row and shard as the lower-case one: T1 (rule), T1c, and Task 5's shard prefix
  (`slice_for_app` case-insensitive); Task 4's `apps[]` ownership match is case-insensitive
  (assert one upper-case dependency guid in T9f's DD arm if DD's manifest allows; else note it).
- **A workspace with no `app.json` id** (`primary_app: None`, empty `apps[]`): every row is
  primary (T1's `None` arm), and an empty `apps[]` still loads (Task 6 adds that arm to T7b's
  valid-file check: an empty-`apps` v4 file loads).
- **A shard handed to `alsem diff`:** the primary shard loads and diffs; a dependency shard is
  refused with the hint (T7b's fifth copy).
- **A fatal preflight under `--fail-on`:** exit 1, stdout unchanged (T6b; Task 6 also runs it
  with `--fail-on low`).
- **A cross-app snapshot of a workspace with no dependency** equals the single-app one except
  `analysisUniverse` (T10).

---

## Task 1 — `Role`, the one rule, at every site

**Files:**
- Create: `src/program/model/role.rs` (from the red copy: types + T1; real bodies).
- Modify: `src/program/model/mod.rs` (`pub mod role;`), `src/program/model/projection.rs` (T1c
  only; `role_of` itself is kept), `src/engine/l5/registry.rs` (`:449-456`, `:524-543`; T2),
  `src/engine/l4/capability_cone.rs` (`:3866-3880` builds `dep_routine_ids` from
  `routine_role`; delete `primary_app_guid` at `:3743`, `:4000` and its census bucket `:4035`),
  `src/engine/gate/run.rs` (`:244-249`, `:414-421`; an unknown object stays `false`),
  `src/program/model/coverage.rs:243-250`, `src/engine/l5/digest.rs:3700-3712`,
  `src/engine/l4/effect_query_cli.rs:355-360`, `src/engine/l5/selector_index.rs:80-95` (calls
  `role_of_stable_id`), test helpers `src/engine/l4/exec_relation.rs:353-359`,
  `src/engine/l5/test_support.rs:635`.
- Not touched: `projection.rs:97-111` (`role_of`, kept), `detectors/d1.rs:1993-1998` (kept,
  spec O1), the `dep:` location filter `registry.rs:544-558`.

**Interfaces (produces):**
```rust
pub enum Role { Primary, Dependency }            // Clone, Copy, Debug, PartialEq, Eq
impl Role { pub fn as_str(self) -> &'static str } // "primary" | "dependency"
impl Model {
    pub fn role_of_app(&self, app_guid: &str) -> Role;   // eq_ignore_ascii_case; no primary => Primary
    pub fn routine_role(&self, r: &ModelRoutine) -> Role;
    pub fn object_role(&self, o: &ModelObject) -> Role;
}
pub fn role_of_stable_id(primary: Option<&str>, stable_id: &str) -> Role; // "<guid>:" prefix, case-insensitive
```
The registry's maps keep `&str` values (`Role::as_str()`).

- [ ] **Red.** Bring in `role.rs` (T1 `role_rule_hand_stated`), the `mod.rs` line, T1c
  (`projection_role_of_agrees_with_rule` + its `copy_dir` helper) and T2
  (`cross_app_scope_uses_role` + `role_probe`) from the red copy, with stub bodies. Run
  `cargo test -p al-sem --lib role_rule_hand_stated projection_role_of_agrees_with_rule`
  (two runs): both fail `not implemented: #220`. T2 passes (expected, see deviations).
- [ ] **Green.** Write the real bodies, then convert every site listed above. Run the three
  tests: pass.
- [ ] **A8.1 grep.** `rg -n 'app_guid.*(eq_ignore_ascii_case|to_ascii_lowercase)|eq_ignore_ascii_case\(g\)|eq_ignore_ascii_case\(p\)' src`
  hits only `role.rs`, `projection.rs:97-111` and spec A8.1's listed normalisations
  (`compiler_oracle.rs:313`, `events.rs:254`, `program_calls.rs:386`, `d13.rs:82-83`,
  `program_calls_adapter_tests.rs:1505,1641`). `rg -n primary_app_guid src/engine/l4` = 0.
  `git diff master -- src/engine/l5/detectors/d1.rs` is empty.
- [ ] **Discrimination proofs** (Edit break → fail line → revert → pass), spec §5:
  - T1: case-sensitive compare in `role_of_app` → the upper-case row reads `Dependency`; drop
    the `None` arm → `Dependency`; drop the `:` check in `role_of_stable_id` → `<primary>x`
    reads `Primary`.
  - T1c: `role_of` treats `dep:` as primary → fails; `role_of` drops its lowercase step → the
    upper-case manifest guid fails.
  - T2: `role_of_app` always returns `Primary` → both dependency-anchored findings are kept
    under `--scope primary`, fails.
- [ ] **CDO identity spot-check (spec §13.1).** Build `cargo build --profile release-fast` on
  the task commit's parent and on the task tree; `alsem analyze <WS> --format sarif --scope
  primary` (default cross-app) on both; `cmp` the two SARIF files: identical. Any difference is
  F8 (a defect: stop). Record both SHA-256s in the ledger `## Gate results`.
- [ ] Gate, commit (`role.rs`, `mod.rs`, `projection.rs`, `registry.rs` and the converted
  files), `check-diff`.

## Task 2 — Schema 4 shape and the golden move

The ONE golden-moving task. Single-app only in production, so the moves are exactly: the header
(`schemaVersion` 4, `analysisUniverse: "single-app"`), `apps[].role: "primary"`, the inventory
document, and `diff/PROVENANCE.md`.

**Files:**
- Modify `src/engine/l5/snapshot_full.rs`: `SNAPSHOT_SCHEMA_VERSION = 4` (the ONE constant);
  `analysisUniverse` inserted right after `generatedAt`; `FullSnapshotOptions.universe:
  SnapshotUniverse<'a>`; `SnapshotUniverse { SingleApp, CrossApp(CrossAppUniverse) }`,
  `CrossAppUniverse { base: &R3a5CrossAppBase }` (private field), `SnapshotUniverse::of(&AnalysisTarget)`;
  in `CrossApp`, `assert!(std::ptr::eq(resolved, &base.resolved), ...)` at the top of
  `compose_full_snapshot`; `project_apps` (`:381-411`) writes `role` last in each row,
  primary first, then (cross-app) every dependency app owning an `identities` row sorted by
  `locale_compare` of guid, ownership matched case-insensitively, guid spelled as in the
  manifest, plus the F22 `debug_assert!`; `INVENTORY_SCHEMA_VERSION = "1.2.0"`;
  `build_inventory_doc` puts `analysisUniverse` in `payload` next to `apps`. (No row filter yet:
  Task 4 classes `routineInventory` and limits `apps[]` to SERIALIZED identities.)
- Modify `src/engine/l4/capability_cone.rs`: `DependencyApp.publisher` (from `AppId.publisher`,
  `program_calls.rs:1002-1007`); `R3a5CrossAppBase.dependency_apps` (cloned before
  `resolved_app_versions`, `:3991-3995`).
- Modify `src/engine/gate/snapshot_deserialize.rs`: delete the private constant (`:38`), use
  `snapshot_full::SNAPSHOT_SCHEMA_VERSION`; the refusal message becomes
  `deserializeSnapshot: snapshot schemaVersion <n> is not supported by this build (v4); re-create it with 'alsem fingerprint'`;
  rewrite `deserialize_json_raw`, `deserialize_json_enveloped`,
  `normal_gz_snapshot_round_trips_under_cap`, `roundtrip_*` to build trees with the constant,
  `analysisUniverse` and roles; `wrong_schema_version_errors` (red copy) refuses 3.
- Modify production callers to pass `universe: SnapshotUniverse::SingleApp`:
  `src/engine/l5/fingerprint_cli.rs`, `src/engine/gate/diff/cli.rs` (`:140`).
- Modify tests (compile-only `universe` field, O11): `tests/cli/cli_b_fingerprint_differential.rs:147`,
  `tests/cli/cli_b_snapshot_differential.rs:163`, `tests/cli/cli_b_diff_differential.rs:201`,
  `tests/gap/gap_issue100_testfield_capability.rs:82`.
- Modify `tests/cli/cli_b_diff_differential.rs`: `snapshot()` (`:489-500`) uses the constant,
  the universe key and a `role` on app rows; `oracle_preflight_schema_version_mismatch_is_fatal`
  keeps 99 on the old side, the constant on the new side (`:551`);
  `oracle_preflight_app_identity_mismatch_is_fatal`'s app rows gain `role`.
- Modify `tests/cli/cli_p1_inventory.rs` (T11, red copy).
- Create `tests/cli/cli_snapshot_schema4.rs` with the shared helpers this task's tests use and
  T7 `diff_refuses_schema_3`, T12 `cross_app_universe_binds_its_model`; add
  `mod cli_snapshot_schema4;` to `tests/cli/main.rs`. Later tasks append their tests and the
  helpers those need.
- Hand-edit (Edit, three edits each, spec D6) the 12 `tests/cli-b-goldens/diff/snapshots/*.snap.json`:
  `"schemaVersion": 4`; `"analysisUniverse": "single-app"`; `"role": "primary"` on the one
  `apps[]` row. Add one paragraph to `tests/cli-b-goldens/diff/PROVENANCE.md` saying the inputs
  were upgraded by hand from schema 3 to 4 with exactly these three edits for #220.
- Regenerated: the 62 schema-3 JSON files and 86 CBOR files of A8.9, by `scripts/check-goldens --regen`.

- [ ] **Red.** Bring in T7, T12, T11's edits and `wrong_schema_version_errors`; run each: T7
  fails on `msg.contains("schemaVersion 3")`; T12 `composing A's model with B's universe must
  panic`; T11 `left: "1.1.0"`; `wrong_schema_version_errors` `schemaVersion 3 is refused`.
- [ ] **Green: code.** Implement the source changes above. Run the four tests: pass.
- [ ] **Green: inputs.** Hand-edit the 12 inputs and PROVENANCE. Run
  `cargo test --test cli cli_b_diff_differential::` : every diff OUTPUT golden still byte-equal
  (F15: a moved output means the edit is wrong).
- [ ] **Regen.** `scripts/check-goldens --regen > /work/bench/160-8/regen-T2.log 2>&1; echo $?`,
  then `git status --short tests/` lists only A8.9's files (cli-b `snapshot/`, `fingerprint/`,
  `diff/snapshots/`, `diff/PROVENANCE.md`) and nothing else; no digest, prove,
  fingerprint-query, `.human.txt` or diff output file. Dispatch the `golden-diff-triager` agent
  on the regen diff; every change must be header- or `role`-shaped. Then
  `scripts/check-goldens` (no regen) exits 0 and `cargo test --test cli
  cli_workspace_fingerprint_closure` passes unchanged (A8.11; a failure is F5).
- [ ] **Discrimination proofs.**
  - T7: accept 3 in the loader (`version == 3 || ==` constant) → loads, fails.
  - T12: delete the `ptr::eq` assert → composes silently, fails.
  - T11: set `INVENTORY_SCHEMA_VERSION` back to `"1.1.0"` → fails; drop `analysisUniverse`
    from the payload → fails.
  - `wrong_schema_version_errors`: same break as T7 → fails.
- [ ] `rg 'SNAPSHOT_SCHEMA_VERSION: i64' src` = 1 hit (A8.4).
- [ ] Gate (here `goldens` moves are expected and must equal the regen), commit every file
  above in one commit, `check-diff`.

## Task 3 — Live gaps, reach over the cone (small)

**Files:**
- Modify `src/engine/l5/snapshot.rs`: `derive_analysis_gaps(resolved: &Model, base)` (gains
  `resolved`); `is_dependency_role` = `resolved.routine_role(r) == Role::Dependency` (stub
  `:2433-2435` deleted); `SnapshotAnalysisGap.role: String` serialized last, after `detail`:
  `"primary"` for a primary `parse-incomplete`, else `"dependency"`. Kinds, subjects, sort and
  dedup unchanged; no reach filter in memory (I8).
- Modify `src/engine/l5/snapshot_full.rs`: in `CrossApp` only, `reach` = BFS over
  `base.rel.cone()` (`TypedEdge.from → to`, internal ids) from every primary routine of the
  model; filter the composer's local `core.analysis_gaps`: keep every primary gap; keep a
  dependency gap if its routine subject's row is in `reach`, or its app subject (bare guid) owns
  a row in `reach` (`object_id` first segment, case-insensitive). `debug_assert!` that every
  kept `symbol-only-boundary` app has `has_source == false` in `base.dependency_apps` (F21).
  `fn reach(base: &R3a5CrossAppBase) -> HashSet<String>` is reused by Task 4.
- Every caller of `derive_analysis_gaps` (grep) passes the model. A lib test reading in-memory
  gaps of a cross-app model that moves (F11) is rewritten here and named in the commit message.
- Append T3 `gaps_reachable_only`, T4 `parse_incomplete_gap_filter` (+ `write_t4`), T4c
  `gap_reach_follows_dependency_calls`, T5 `symbol_only_callee_has_its_gap` and their helpers
  (`cone_reach`, `cut_reach`, `bfs`, `gaps`, `memory_gaps`, `hand_state_parse_incomplete`,
  `dd_dir`, `r5b`) to `tests/cli/cli_snapshot_schema4.rs`.

- [ ] **Red.** Run the four: T3 `precondition: in-memory symbol-only-boundary for dddddddd-…0001: []`;
  T4 `W is gapped with role primary`; T4c `Inner's parse-incomplete gap is serialized` (no
  `role`); T5 `dddd…'s symbol-only-boundary gap: []`.
- [ ] **Green.** Implement; the four pass. If a test asserts on a key other than
  `analysisGaps` and needs Task 4's filter, move that test to Task 4 and say so in the ledger.
- [ ] **Discrimination proofs** (spec §5):
  - T3: skip the reach filter → `eeee…` appears; stub `is_dependency_role` back to `false` →
    `dddd…` missing.
  - T4: no reach filter → `R` appears; hard-code `role: "dependency"` → `W`'s role fails.
  - T4c: BFS over `rel.primary()` (the cut) instead of `rel.cone()` → the gap disappears.
  - T5: stub back to `false` → gap missing.
- [ ] Gate (no golden moves: cli-b `analysisGaps` are all `[]`, single-app rows all primary),
  commit, `check-diff`.

## Task 4 — Key classes, the typed filter, the closure (large)

**Files:** `src/engine/l5/snapshot_full.rs` (main), `src/engine/l5/snapshot.rs` (value-source
walk helpers if they belong beside `SnapValueSource`), `tests/cli/cli_snapshot_schema4.rs`.

**Interfaces (produces; names as in the red stubs, signatures may change with their tests):**
```rust
pub enum KeyClass { Header, Input, Inventory, Behaviour, Gap, Evidence }
pub const SNAPSHOT_KEYS: &[(&str, KeyClass)];               // every top-level key, insertion order (spec §3.3, §4.4 table)
pub enum Domain { Identity, App, Operation, Callsite, Frame, Edge }
pub const REFERENCES: &[(&str, Option<&str>, &[(&str, Domain)])]; // key, defining field, reference fields (spec §4.4 list)
pub const NON_REFERENCE_FIELDS: &[&str];                   // spec §4.4 list, completed by census
pub struct ClosureViolation { /* key, field path, value, domain */ }
pub fn closure_violations(tree: &CborValue, resolved: &Model, universe: &SnapshotUniverse) -> Vec<ClosureViolation>;
pub(crate) fn filter_by_class(core: &mut CapabilitySnapshot, permission_facts: &mut CborValue,
                              primary_app_guid: Option<&str>, reachable: &HashSet<String>);
```

- In `SingleApp` `filter_by_class` is not called (every row primary, P3).
- In `CrossApp`, on the composer's local typed rows before `to_cbor_value` (spec §4.4, O7):
  `derive_contracts`/`derive_schema`/`derive_permissions` skip dependency rows by
  `object_role`/`routine_role`; then the six ordered steps of spec §4.4 (subject filter;
  resolve references against UNFILTERED rows; Evidence rows of a primary routine or referenced;
  their references and frame tables; `identities` = every primary id + every referenced one;
  `apps` = primary + owners of kept identities + apps referenced by a kept gap, no row for an
  unknown guid, O4N1). Matching rules per domain exactly as spec §4.4's table (internal
  `<guid>/table/N[/…]` → `stable_table_id` of its first three segments; never a `/`→`:`
  rewrite; operation/callsite/edge ids verbatim; a defining field is never a reference). Value
  sources read by `kind`: `table-field` `tableId`, `constant-var` `initializer` recursively.
- `routineInventory` in `build_inventory_doc` classed Inventory by `stableRoutineId`.
- `closure_violations`: per domain, with `EXTERNAL_IDENTITY_SHAPES` (literal `"unknown"`; an id
  rooted in a guid that is neither primary nor in `dependency_apps` and equals no model routine
  id) and `parentFrameId = -1` the only exceptions.

- [ ] **Red.** Append T9a `key_class_inventory`, T9b `key_class_behaviour`, T9d
  `key_class_evidence`, T9e `key_class_input`, T9f `key_class_header_and_apps_order`,
  `closure_fields_are_classified_and_closed`, `closure_reports_each_dangling_domain`,
  `composed_keys_are_classed`, T10 `no_dependency_cross_equals_single` (+ `path_matches`,
  `is_reference`, `classified`, `leaves*`, `id_shaped`, `evidence_ids`) to the cli member; add
  T9g `evidence_follows_dependency_witness`, `closure_nested_references_keep_their_rows`,
  `closure_literal_keeps_nothing` and their row builders (`p_routine`, `d_routine`, `order`,
  `frames`, `op_row`, `cs_row`, `fact`, `core_with`, `occurrences`) to `snapshot_full.rs`'s
  tests, and `#[cfg(test)] RoutineOrderFrames::from_entries` to `snapshot.rs`. Run each; record
  the ledger's red line for each.
- [ ] **Green.** Implement. Fill `REFERENCES`/`NON_REFERENCE_FIELDS` from the census the
  classification test prints; a field in neither list is classified from its serde struct
  (F17), never by reading every string. A dangling value that is pre-existing on the base is
  F25 (recorded discovery, one shape added), never a domain-wide relaxation. All 13 pass.
- [ ] **Discrimination proofs** (spec §5 rows; one ledger row each):
  - T9a, T9b: replace the filter with identity (keep all rows) → dependency rows appear.
  - T9d: break 1 emit every Evidence row → `eeee…` ids and `Inner`'s operation appear; break 2
    keep primary only → `Outer` missing; break 4 drop primary rows from Evidence → `TouchCust`'s
    operation missing. (Break 3 lives in T9-closure; the `Outer` call-site arm is vacuous until
    step 6, see deviations.)
  - T9e: filter `inputs` as Inventory → dependency `.app` rows vanish.
  - T9f: sort `apps[]` by guid only → `dddd…` first in R5B-F.
  - T9g: match operation ids through the identity mapping → rows dropped; skip step (4) →
    frame table and identity missing.
  - T9-closure: breaks 1-8 of spec §5 (read every `Text`; drop `tableId`; `/`→`:` table
    rewrite; `stableId` as reference; only `resourceArgSource`; no `initializer` recursion;
    drop `witnessFirstHopOperationId` from `permissionFacts`; allow any unresolved value per
    domain, one run per domain).
  - `composed_keys_are_classed`: insert a test key without a class → fails.
  - T10: write gap `role` only cross-app → fails; Evidence keeps referenced ids only → `Unused`
    vanishes cross-app, fails.
- [ ] Gate (no golden moves; the single-app path writes the same bytes as after Task 2),
  commit, `check-diff`.

## Task 5 — Shards by role, shard closure (small)

**Files:** `src/engine/l5/snapshot_full.rs` (`serialize_sharded` `:1280-1286`, `slice_for_app`
`:1318-1376`, tests).

- `serialize_sharded` reads each `apps[]` row's `role`; `primary-only` keeps `role ==
  "primary"`; `apps[0]` is never read for a role. Manifest shape unchanged.
- `slice_for_app`'s `<guid>:` prefix compare becomes case-insensitive (O13); its membership
  rules are unchanged. After slicing, close each shard to a fixpoint over the identity,
  operation, callsite and edge domains (reusing Task 4's `REFERENCES` walk); never the frame or
  app domains (no `routineOrderFrames` key in a shard; `analysisGaps` copied whole).

- [ ] **Red.** Add T8 `shards_read_role_not_position` (+ `shard_tree`, `txt`, `map`, `texts`)
  and `slice_for_app_handles_every_classed_key`. T8 fails `left: Some("dependency") right:
  Some("primary")`; the keys test fails only if Task 4's `SNAPSHOT_KEYS` and `slice_for_app`'s
  matched keys differ (if it is already green, record that and prove it with its break).
- [ ] **Green.** Implement; both pass.
- [ ] **Discrimination proofs:** T8 break 1 read `apps[0]` → roles swapped; break 2 close
  identities only → the operation row missing; break 3 one pass instead of a fixpoint → the
  operation's routine identity missing; break 4 also close frames → the shard gains
  `routineOrderFrames`. Keys test: add a key to `SNAPSHOT_KEYS` that `slice_for_app` does not
  handle → fails.
- [ ] Gate: the three `primary.json` shard goldens and the `fingerprint/shards/` files must not
  move (F23, F26). Commit, `check-diff`.

## Task 6 — Preflight: load refusals, universe, role identity, exit 1 (small)

**Files:** `src/engine/gate/snapshot_deserialize.rs` (D5.2), `src/engine/gate/diff/preflight.rs`
(D5.3 `analysis-universe-mismatch` right after `schema-version-mismatch`, same shape as
`app-identity-mismatch` with `kind`, `old`, `new`; D5.4 `primary_app_id` reads the `role ==
"primary"` row), `src/engine/gate/diff/mod.rs` (`DiffEngineResult.fatal: bool`, set on a fatal
preflight, `:145-153`), `src/engine/gate/diff/cli.rs` (exit 1 when `fatal`, before the coverage
and `--fail-on` gates, `:299-307`; stdout unchanged), `tests/cli/cli_snapshot_schema4.rs`.

- D5.2 refusals, each naming the field: `analysisUniverse` missing or not `single-app` /
  `cross-app`; an `apps[]` row without `role` or with another value; a non-empty `apps[]`
  without exactly one `primary` row; a dependency shard gets the hint `diff the full snapshot or
  the primary shard`. An empty `apps[]` stays accepted.
- If any existing test or golden input is refused (F20), upgrade it like D6 and name it; never
  relax the rule.

- [ ] **Red.** Add T6 `diff_refuses_mixed_universes`, T6b `fatal_preflight_exits_one`, T7b
  `diff_refuses_bad_universe_or_roles` (+ `vendored`, `v4_copy`, `diff`, `loads`,
  `diag_kinds` if not yet added) to the cli member, and T8b `preflight_identity_reads_role` to
  `preflight.rs`. Add to T7b one valid-file arm: an empty-`apps[]` v4 copy loads (Review
  Focus). Red lines: T6 (now loads) no `analysis-universe-mismatch`; T6b `left: 0 right: 1`;
  T7b `no-universe` loads; T8b no diagnostic.
- [ ] **Green.** Implement; all four pass. Also run T6b's pair with `--fail-on low`: exit 1.
- [ ] **Discrimination proofs:** T6 drop the universe check → findings computed, exit 0; T6b
  drop the `fatal` gate → exit 0; T7b drop each of the five checks in turn → that copy loads
  (five rows); T8b read `apps[0]` → no diagnostic.
- [ ] `cargo test --test cli cli_b_diff_differential::` green: the in-process oracles assert
  diagnostics, not exit codes; no diff output golden moves.
- [ ] Gate, commit, `check-diff`.

## Task 7 — CHANGELOG, `allowlist.py`, ledger (small)

**Files:** `CHANGELOG.md`; `/work/bench/160-8/allowlist.py` (outside the repo, not committed);
the ledger rows.

- [ ] **CHANGELOG** under `## [Unreleased]`:
  - **Changed:** snapshot schema 4 (`schemaVersion` 4; schema 3 refused at load with the
    re-create remedy); routine-inventory document 1.2.0 (`apps[].role`,
    `payload.analysisUniverse`, `routineInventory` primary-only cross-app); a fatal diff
    preflight now exits 1 (`app-identity-mismatch` and strict `alsem-version-mismatch` exited 0);
    `diff` refuses a missing/unknown `analysisUniverse`, role-less or unknown-role `apps[]`
    rows, not exactly one primary, and a dependency shard; one role rule (`Role`) replaces the
    per-site comparisons; analysis gaps live (were stubbed off for dependency rows).
  - **Added:** `analysisUniverse` header; `role` on `apps[]` and gap rows; key classes
    (`SNAPSHOT_KEYS`, `REFERENCES`) with the cross-app typed-row filter and the closure check;
    `analysis-universe-mismatch` preflight.
  - **Fixed:** shards and diff preflight read the primary app by position (`apps[0]`), not by
    role; shard slices now keep the identity, operation, call-site and edge rows their kept
    rows reference; the shard guid prefix compare is case-insensitive.
- [ ] **`allowlist.py`** (spec §8 Part 0): `python3 -I /work/bench/160-8/allowlist.py
  <P-tree>/tests/cli-b-goldens <H-tree>/tests/cli-b-goldens`, `P` = the branch base (after a
  rebase, the new base), `H` = HEAD, each extracted with `git archive <sha> tests/cli-b-goldens
  | tar -x -C <dir>`. It imports `cbordec.py` (P4b), keeps key order for JSON and CBOR, and
  exits 2 on: a changed file outside A8.9's list; a `PROVENANCE.md` diff that is not one added
  paragraph; on `H` any `schemaVersion`/`snapshotSchemaVersion`/manifest `schemaVersion` ≠ 4,
  `analysisUniverse` ≠ `"single-app"`, an `apps[]` row whose `role` ≠ `"primary"` or that is not
  the only row, a gap row without `role`; `H` minus exactly those keys ≠ `P`; a key order on
  `H` (minus added keys) ≠ `P`'s, or `analysisUniverse` not right after `generatedAt` in CBOR
  and `manifest.json`, or `role` not last in its row; a diff input that differs from `P` by
  more than D6's three edits; a missing or unparseable file. Exit 0 otherwise.
- [ ] **Mutation checks** (before use): four hand-made `H` copies under
  `/work/bench/160-8/allowlist-mut/` (a wrong universe value; a missing `role`; a moved key; an
  unlisted changed file), each must exit 2; record each exit code.
- [ ] **Run** it on `P` vs `H`: exit 0. Record its SHA-256 (`sha256sum`), its output and the
  four mutation results in the ledger `## Gate results` (and later the step's numbers file).
- [ ] Ledger: every discrimination row filled; `## Gate results` with each task's gate log
  exit codes. Leave the Task 0 stash in place (never `git stash drop` without the operator's
  word); confirm every red test of the ledger table is now committed by name
  (`git grep -n 'fn <name>'` per test).
- [ ] Gate, commit `CHANGELOG.md` (and the ledger only if the flow commits it), `check-diff`.

Spec §8 Parts 1-2 (CDO SARIF ×4, histogram SHA, digest, query effects, `cdo-gate`, the CDO
fingerprint through `allowlist.py`) run once after the final panel by the conductor, not here.

## Acceptance map (spec §6)

| Item | Task |
|---|---|
| A8.1 Role + grep | 1 |
| A8.2 live gaps with reach | 3 |
| A8.3 key classes | 4 (T12 in 2) |
| A8.4 schema 4, one constant | 2 |
| A8.5 `apps[]` by role | 2, 4 (T9f), 5 (T8), 6 (T8b) |
| A8.6 preflight | 2 (T7), 6 |
| A8.7, A8.9 cli-b allow-list, narrowed golden move | 2 (regen), 7 (`allowlist.py`) |
| A8.10 `cli_p1_inventory` | 2 (T11) |
| A8.11 `cli_workspace_fingerprint_closure` unchanged | 2 |
| A8.12 R2d key list, not deliverable | — (T9-keys, 4) |
| A8.13 no bench | — |
| Step 6/9 carry-overs (spec §12) | not in this plan |
