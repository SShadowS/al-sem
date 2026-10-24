# Closed-enum interface dispatch (#264) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** An interface call whose variable provably holds a value of a non-extensible SOURCE enum gets a closed candidate set (`SetCompleteness::Complete`, `DispatchMeta::closed_over`), loses its `interface-open-world` uncertainty, and so no longer makes d35 report an "unknown" Commit.

**Architecture:** A per-enum fact (`ObjectNode::closed_enum` + `enum_impls`) is built from lowered enum values; a new resolver module proves, per call site, that every write to a routine-local interface variable comes from closed enums (allowlist + identity typer) and that every mapped implementation is a loaded route; the edge becomes `Complete`; the model adapter, combined graph, snapshot and unresolved-cone ledger read one `closed_over` field. Routes, shapes and histograms do not change.

**Tech Stack:** Rust (al-sem crate, `crates/al-syntax`), cargo-nextest gates, alc 18.0.41 for fixture probes.

**Spec:** `docs/superpowers/specs/2026-10-24-issue-264-closed-enum-interface-dispatch-design.md` (revision 4, commit 303868e2). Section refs (§1–§6, T-numbers, M1–M4, F1–F6) are the spec's.

## Global Constraints

- Worktree `/work/al-sem-issue-264-a1`; `export TREE_SITTER_AL_PATH=/work/alsem/tree-sitter-al` in every shell.
- Package name `al-sem` (hyphen): `cargo test -p al-sem --lib <filter>`.
- Per-task gate: `scripts/ci-steps task-affected` (redirect to a log and grep it; never pipe a gate through `| tail`).
- Edit files with Edit/Write only, never a script; format with `rustfmt <file>` per file, never `cargo fmt`.
- Every closed claim fails OPEN on doubt (spec Goal): `closed_enum` defaults to `false`; any unmodelled shape → `None`.
- Routes, `DispatchShape`, the `--program-call-graph-stats` histograms and every CDO ratchet stay byte-identical (F1). No golden regen (F4): `scripts/check-goldens` must pass WITHOUT `--regen`.
- The shared receiver typing is not changed (`receiver.rs:1612-1616`, the `FromInteger` arm at `:1954-1962`): spec Non-goals.
- Symbol-only enums (apps without source) stay open: `abi_ingest` sets `closed_enum: false`; no `symbol_reference.rs` change (Discovery D1).
- `DispatchMeta.closed_over: Vec<String>` is part of the derived `Debug` (the acceptance tests read `format!("{dm:?}")`); its elements are model object ids (`model_object_id`), sorted, the same format as `enum_implementers`.
- Every new test gets a discrimination proof (`discrimination-proof` skill): break with Edit, run red, revert with Edit, run green; record both outcomes in `.agent/issue-264/ledger.md` under `## Discrimination proofs`.
- Commit per task, staging only that task's paths (never `git add -A`); commit message ends with `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`.
- Anything handed to a subagent runs on `model: opus`.

## Review Focus

1. An enum variable declared by NUMBER (`K: Enum 50102`) or a namespace-qualified type text: a reasonable user expects the same closed result as by name. Test owned by Task 4 (`i264_rf1_enum_by_number_and_namespace_closes`).
2. Case and quoting differences between declaration, write and call (`strategy := …; "Strategy".NextKm(…)`): the occurrence walk must fold and unquote, else a write hides. Test owned by Task 3 (`i264_rf2_case_and_quote_variants_are_one_variable`).
3. The receiver used inside a condition or nested block (`if S.NextKm(1) > 0 then`, inside `repeat … until`): allowed occurrences, the site stays closed. Test owned by Task 3 (`i264_rf3_member_call_in_condition_stays_closed`).
4. An enum implementing TWO interfaces, dispatch over the second one: closed only when every value maps that interface (or a Default exists). Test owned by Task 2 (`enum_closure_maps_each_implemented_interface`).
5. A closed enum whose value is in a `#if` arm only (`#if CLEAN27 value(1; X) { Implementation = … } #endif`): the arm's implementation must still be required as a loaded route. Test owned by Task 1 (`enum_value_inside_if_wrapper_is_lowered`) and Task 2 (`enum_closure_includes_conditional_values`).

## Fixture facts (probe P7, alc 18.0.41, `/work/bench/264/probes/fx-*/`)

- T1, T2, T3 (all 7 RHS shapes and the A4 shapes), T4, T22, T27 compile clean.
- **T21 cannot exist in a compiling closure:** alc rejects two apps in one closure declaring enum `"Demo Kind"` with **AL0197** ("An application object of type 'Enum' with name 'Demo Kind' is already declared by the extension …"; name alone, ids 50102/70002). The identity typer stays as defence in depth for input al-sem parses but alc would reject. T21 is a stated-precondition defensive test; its doc comment (and the existing `i264_t21_same_name_enum_resolves_in_the_declaring_app` control's doc) must cite AL0197. Note for readers of the spec: its S1 rationale is a defensive guard, not a real-world compiled shape.
- **T9b compiles** (field `Kind` + procedure `Kind()` on the table, and on a tableextension) with warning **AL0486** ("will become an error in a future release"). Keep the shape; cite AL0486 in the test doc.

## Existing acceptance tests (uncommitted, `tests/cli/cli_d35_unknown_commit.rs`, `#264` section)

Red now: `i264_t1_…`, `i264_t2_…`, `i264_t4_…`, `i264_t3_rhs_shapes_are_closed`, `i264_t22_…`, `i264_t27_…` (cross-app half). Green controls: `i264_a3_…`, `i264_a4_…` (T5, T6, T6b, T7, T8), `i264_t21_…`. Helpers to reuse in every new d35-level/model-level test: `cg_files`, `cg_kind`, `CLOSED`, `single_app_model`, `cross_app_model`, `interface_site` → `Site`, `assert_closed(site, candidates, label)`, `assert_open(site, label)`, `impl_next_km`, `interface_completeness(ws, routine_lc) -> SetCompleteness`, `write_dep264`, `d35_findings`, `analyze`. New resolver-level tests go in the same `#264` section and assert `interface_completeness(..) == SetCompleteness::Complete` (closed) or `matches!(.., SetCompleteness::Partial { .. })` (open). Commit this file with Task 3 (the first task that turns any of it green), keeping the red tests red-but-documented until Task 5.

---

### Task 1: Lower enum values (`ObjectDecl.enum_values`)

**Files:**
- Modify: `crates/al-syntax/src/ir/decl.rs:9-58` (field + struct)
- Modify: `crates/al-syntax/src/lower/mod.rs:320-335` (collect), tests module of the same file
- Test: `crates/al-syntax/src/lower/mod.rs` tests

**Interfaces:**
- Produces:
  ```rust
  pub struct EnumValueDecl {
      pub name: String,               // unquoted, as written
      pub implementations: Vec<String>, // raw `Implementation` property value, one per #if arm, trimmed
      pub origin: Origin,
  }
  // ObjectDecl
  pub enum_values: Vec<EnumValueDecl>, // Enum objects only; document order; values inside #if wrappers included
  ```

- [ ] **Step 1: Write the failing tests** in `lower/mod.rs` tests:
  - `enum_values_are_lowered_with_implementations`: `enum 50102 "K" implements "I", "J" { value(0; A) { Implementation = "I" = "Impl A", "J" = "Impl J"; } value(1; "B C") { } }` → `enum_values.len() == 2`; `[0].name == "A"`, `[0].implementations == vec![r#""I" = "Impl A", "J" = "Impl J""#]`; `[1].name == "B C"`, `[1].implementations.is_empty()`.
  - `enum_value_implementation_if_arms_give_one_entry_per_arm`: value body `#if X Implementation = "I" = "P"; #else Implementation = "I" = "Q"; #endif` → two entries `"I" = "P"`, `"I" = "Q"`.
  - `enum_value_inside_if_wrapper_is_lowered` (Review Focus 5): `#if CLEAN27 value(1; X) { Implementation = "I" = "P"; } #endif` → the value is present.
  - `non_enum_objects_have_no_enum_values`: a codeunit and a table → empty.
- [ ] **Step 2: Run** `cargo test -p al-syntax --lib enum_value` → FAIL (no field).
- [ ] **Step 3: Implement** `fn collect_enum_values(node: RawNode, source: &str, out: &mut Vec<EnumValueDecl>)` mirroring `collect_properties`' `#if`-wrapper recursion (`lower/mod.rs:791-800`); for each `RawKind::EnumValueDeclaration` read field `value_name` (unquote with `ident_text`) and run `lower_property` over its body's `property` nodes, keeping those named `implementation`. Call it in the object-body loop (`:323-327`) only when `kind == ObjectKind::Enum`; set the field at the single construction (`:335`).
- [ ] **Step 4: Run** the four tests → PASS; `cargo test -p al-sem --test l2_ir` → PASS (F5: `tests/ir-l2-goldens/` must not move; if it does, STOP per F5).
- [ ] **Step 5: Lowerer review.** Dispatch the `lowerer-reviewer` agent (model opus) on the diff; resolve every finding before the gate.
- [ ] **Step 6: Gate** `scripts/ci-steps task-affected > /tmp/claude-1000/…/t1-gate.log 2>&1; echo $?` → 0.
- [ ] **Step 7: Commit** `crates/al-syntax/src/ir/decl.rs crates/al-syntax/src/lower/mod.rs` — "al-syntax: lower enum values with their Implementation properties (#264)".

### Task 2: The enum fact (`ObjectNode::closed_enum`, `enum_impls`), dedup, pack

Owns **T9, T15, T16, T17, T31, T33**, Review Focus 4 and 5 (fact half).

**Files:**
- Modify: `src/program/node_extract.rs:127-183` (fields), `:759` (fill), tests module
- Modify: every `ObjectNode {` literal (`grep -rn "ObjectNode {" src tests`; add `closed_enum: false, enum_impls: Vec::new()`)
- Modify: `src/program/abi_ingest.rs:515-540`
- Modify: `src/program/build.rs:307-308`, `:370-371`
- Modify: `src/program/pack/mod.rs:65-71` (+ its tests)
- Test: `tests/cli/cli_d35_unknown_commit.rs` (T9 only)

**Interfaces:**
- Consumes: Task 1 `ObjectDecl.enum_values`.
- Produces:
  ```rust
  // ObjectNode
  pub closed_enum: bool,                         // true only for a provably closed Enum (§1)
  pub enum_impls: Vec<(SharedStr, ObjectRef)>,   // (interface folded, implementing codeunit); empty unless closed_enum
  // node_extract.rs
  pub(crate) fn enum_closure(obj: &al_syntax::ir::ObjectDecl, parse_incomplete: bool) -> (bool, Vec<(SharedStr, ObjectRef)>);
  fn parse_impl_pairs(raw: &str) -> Option<Vec<(String, String)>>; // `"I" = "C"[, "J" = "D"]`, commas outside quotes; None on any malformed pair
  // build.rs
  fn dedup_objects(objects: &mut Vec<ObjectNode>); // equal ids: AND closed_enum, union enum_impls, clear enum_impls when false
  ```

- [ ] **Step 1: Write the failing tests** (node_extract tests parse AL text with `al_syntax::parse` and call `enum_closure`):
  - `enum_closure_absent_extensible_is_closed`: CG enum without `Extensible` → `(true, [("demo strategy", Name "Demo Impl")])`.
  - `enum_closure_false_is_closed` / `enum_closure_true_is_open` (`Extensible = true` → `(false, [])`) / `enum_closure_unknown_value_is_open` (`Extensible = maybe`).
  - T15 `enum_closure_if_arms_disagree_is_open`: `#if X Extensible = false; #else Extensible = true; #endif` → open.
  - T17 `enum_closure_recovered_file_is_open`: same enum, `parse_incomplete = true` → open.
  - `enum_closure_value_without_mapping_or_default_is_open`; `enum_closure_default_and_unknown_are_mapped` (T32 fact half: `DefaultImplementation = "I" = "D"`, `UnknownValueImplementation = "I" = "U"`, value with no Implementation → closed, impls contain `D` and `U`).
  - Review Focus 4 `enum_closure_maps_each_implemented_interface`: `implements "I", "J"`, every value maps both → closed with entries for both; drop the `"J"` pair from one value and no `DefaultImplementation` for J → open.
  - Review Focus 5 `enum_closure_includes_conditional_values`: a value inside `#if` → its implementation is in `enum_impls`.
  - T16/T33 `dedup_objects_ands_closed_enum_and_clears_impls` (build.rs tests): two `ObjectNode`s with one id, `(true, [x])` and `(false, [])` → one node, `closed_enum == false`, `enum_impls.is_empty()`; order swapped → same.
  - T31 `pack_round_trips_closed_enum` (pack tests): a `PackedFile` holding an `ObjectNode` with `closed_enum: true` and one `enum_impls` entry decodes equal; `assert_eq!(PACK_SCHEMA, 7)`.
  - T9 `i264_t9_symbol_only_enum_stays_open` (cli file): `write_dep264` a dependency whose SYMBOLS declare the enum (`Extensible` absent) and no source; workspace dispatches via a field of that enum → `assert_open` on the cross-app model, and the enum's `ObjectNode.closed_enum == false`. Baseline control; break: set `closed_enum: true` in `abi_ingest` → T9 fails once Task 5 lands (record the proof in Task 5).
- [ ] **Step 2: Run** `cargo test -p al-sem --lib enum_closure dedup_objects pack_round_trips` → FAIL (compile errors for the missing items).
- [ ] **Step 3: Implement** the Produces items. `enum_closure` per spec §1 Source; implementation references via the existing `parse_object_ref_value` (`node_extract.rs:510`) after `strip_namespace`. `abi_ingest` sets `closed_enum: false, enum_impls: Vec::new()`. Replace both `objects.dedup_by(|a, b| a.id == b.id)` with `dedup_objects`. `PACK_SCHEMA = 7` with doc line `/// 7: \`ObjectNode::closed_enum\` / \`enum_impls\` (#264).`
- [ ] **Step 4: Run** the tests → PASS. Discrimination: T15 (take the first `extensible` entry), T16/T33 (plain `dedup_by`; then drop only the clear step), T17 (ignore `parse_incomplete`), RF4 (skip the per-interface completeness check).
- [ ] **Step 5: Gate** `scripts/ci-steps task-affected` → 0.
- [ ] **Step 6: Commit** the files above — "program: closed-enum fact on ObjectNode, AND-dedup, PACK_SCHEMA 7 (#264)".

### Task 3: `closed_enum_sources` (locality, allowlist, clean parse) and the `Complete` edge

Owns **T6 (discrimination), T8b, T11, T18, T19, T20, T23, T24, T25, T26, T28, T30, T32 (route half), T34**, Review Focus 2 and 3. After this task closed sites exist at the RESOLVER level only (model work is Task 5).

**Files:**
- Create: `src/program/resolve/closed_enum.rs` (register `pub mod closed_enum;` in `src/program/resolve/mod.rs`). The spec places the function "in receiver.rs near :1443"; a focused module is used instead because `receiver.rs` is 10k lines — same visibility, same callers.
- Modify: `src/program/resolve/edge.rs` (add `Route::target_object`), `src/program/model/program_calls.rs:1981-1997,2058-2063` (use it)
- Modify: `src/program/resolve/full.rs:134-160` (`SiteFacts`), `:660-736`
- Test: `tests/cli/cli_d35_unknown_commit.rs` (`#264` section); `closed_enum.rs` unit tests (T34)

**Interfaces:**
- Consumes: Task 2 `ObjectNode::closed_enum`, `enum_impls`.
- Produces:
  ```rust
  // edge.rs
  impl Route { pub fn target_object(&self) -> Option<ObjectNodeId>; } // Routine(id) → id.object; AbiSymbol{key} → ObjectNodeId{app: key.app, kind: object_kind_from_abi_type(&key.object_type), key: key.object_node_key()}; else None
  // closed_enum.rs
  pub(crate) struct ClosedEnumCx<'a> {
      pub file: &'a al_syntax::ir::AlFile, pub routine: &'a al_syntax::ir::RoutineDecl,
      pub obj: &'a al_syntax::ir::ObjectDecl, pub from_object: &'a ObjectNode,
      pub graph: &'a ProgramGraph, pub index: &'a ResolveIndex,
      pub surface: &'a DeclSurface, pub with_state: WithState,
  }
  pub(crate) fn closed_enum_sources(cx: &ClosedEnumCx, receiver: ExprId, interface_lc: &str) -> Option<Vec<ObjectNodeId>>; // sorted, dedup
  fn allowlisted_writes(file: &AlFile, body: BlockId, v_lc: &str) -> Option<Vec<ExprId>>;   // §3 rule 3; returns assignment VALUE exprs, ≥1
  fn enum_identity(cx: &ClosedEnumCx, value: ExprId) -> Option<ObjectNodeId>;              // Task 3: the variable arm only; Task 4 adds the rest
  pub(crate) fn enum_covered_by_routes(graph: &ProgramGraph, index: &ResolveIndex, enum_node: &ObjectNode, interface_lc: &str, routes: &[Route]) -> bool; // §2 incl. opus N3
  // full.rs SiteFacts
  pub interface_closed_over: Vec<ObjectNodeId>, // empty = open
  ```

- [ ] **Step 1: Write the failing tests** (resolver level; each builds a CG-style workspace with `cg_files`/`write_files` and asserts `interface_completeness`). Variable-RHS fixtures only (`K: Enum "Demo Kind"; … S := K;`), since the other RHS arms land in Task 4:
  - `i264_t28_conditional_writes_from_closed_variables_are_complete`: `if c then S := K1 else S := K2;` → `Complete`.
  - `i264_t30_closed_and_extensible_writes_stay_open`: `S := KClosed; S := KOpen;` (second enum `Extensible = true`) → `Partial`.
  - `i264_t8b_parenthesized_argument_stays_open`: `S := K; Foo((S));` (value param) → `Partial`. Break: make the walk skip parenthesized call arguments → `Complete`.
  - `i264_t23_foreach_variable_stays_open` (`foreach S in L do`), `i264_t24_any_with_stays_open` (`with Rec do;` anywhere in the routine), `i264_t25_quoted_write_stays_open` (`S := K; "S" := SomeCu;`).
  - `i264_t26_unknown_statement_stays_open`: a clean file whose routine nests `begin … end` deeper than `MAX_LOWER_DEPTH` (128, `crates/al-syntax/src/lower/mod.rs:47`) so the lowerer emits `StmtKind::Unknown`; precondition-assert the file parsed `Clean` and an `Unknown` stmt exists → `Partial`.
  - `i264_t18_recovered_caller_file_stays_open`: a syntax error elsewhere in the caller file → `Partial`.
  - `i264_t11_implementer_without_the_method_stays_open`: a second codeunit `implements "Demo Strategy"` without `NextKm`, mapped by a second value → `Partial`.
  - `i264_t20_unloaded_mapped_implementation_stays_open`: a value maps `"Missing Impl"` (no such codeunit in the program) → `Partial`. Break: skip `enum_covered_by_routes` → `Complete`.
  - `i264_t32_default_and_unknown_implementations_are_required_routes`: Default + UnknownValue codeunits present → `Complete`; Default codeunit absent → `Partial`.
  - `i264_t19_symbol_only_implementer_of_a_source_enum_is_complete`: `write_dep264` a dependency whose symbols hold the implementer; the workspace enum maps it → `Complete` (route `AbiSymbol`). Break: `target_object` returns `None` for `AbiSymbol` → `Partial`.
  - `i264_rf2_case_and_quote_variants_are_one_variable`: declared `Strategy`, written `strategy := K;`, called `"Strategy".NextKm(1)` → `Complete`; plus `S := K; "s" := SomeCu;` → `Partial`.
  - `i264_rf3_member_call_in_condition_stays_closed`: `S := K; repeat if S.NextKm(1) > 0 then exit; until true;` → `Complete`.
  - T34 unit test `enum_covered_by_routes_requires_each_implemented_interface` (in `closed_enum.rs`): hand-built enum `ObjectNode` implementing I and J, `closed_enum: true`, `enum_impls` only J entries; I routes present → `false`. Break: drop the per-interface non-empty check → `true`.
  - T6 discrimination is recorded on the existing `i264_a4_…` control: break "accept parameters in rule 2" → `ParamAssigned` (T6b) goes red, `ParamOnly` (T6) stays green (baseline control, spec T6).
- [ ] **Step 2: Run** `cargo test --test cli cli_d35_unknown_commit::i264_` → the new closed-expectation tests FAIL (`Partial`), open-expectation ones PASS as controls (their discrimination comes after Step 3).
- [ ] **Step 3: Implement** per spec §2, §3 rules 1–3 and rule 4's variable arm (resolve the declared `Enum X` from `from_object.id` via `index.resolve_object_ref(graph, from_object.id.clone(), ObjectKind::Enum, ..)`, `Unique` only, target `closed_enum` and `implements` I), §4. In `full.rs`'s Member arm, when `recv` is `Interface{name_lc}` and `receiver` is `Some`, call `closed_enum_sources`; set `completeness = SetCompleteness::Complete` and `facts_out.interface_closed_over` only when routes are non-empty, none is `RouteTarget::Unresolved` or `Evidence::Unknown`, and `enum_covered_by_routes` holds for every enum. `program_calls.rs` switches to `Route::target_object` (no behaviour change).
- [ ] **Step 4: Run** the tests → PASS; `cargo test --test cli cli_d35_unknown_commit::` → the Task-5 tests (T1, T2, T3, T4, T22, T27) still red for model reasons only, all controls green. Discrimination for every new test (breaks listed above; T23–T26: drop the corresponding allowlist rule; T18: drop the clean check; T28: require exactly one assignment; T30: accept when any write is closed).
- [ ] **Step 5: Gate** `scripts/ci-steps task-affected` → 0; `aldump --program-call-graph-stats` on `/work/bench/256/repro-closed` unchanged versus `/work/bench/256/repro-closed/stats.json` (F1 smoke).
- [ ] **Step 6: Commit** `src/program/resolve/{closed_enum.rs,mod.rs,edge.rs,full.rs} src/program/model/program_calls.rs tests/cli/cli_d35_unknown_commit.rs` — "resolve: prove closed-enum interface sources and mark the edge Complete (#264)".

### Task 4: Identity typer arms with binding guards

Owns **T9b, T9c, T9d, T21 (defensive), T29 (resolver half)**, the resolver half of T2/T3, Review Focus 1.

**Files:**
- Modify: `src/program/resolve/closed_enum.rs` (`enum_identity`)
- Modify: `src/program/resolve/index.rs:924` (owner-returning sibling)
- Test: `tests/cli/cli_d35_unknown_commit.rs`

**Interfaces:**
- Consumes: Task 3 `ClosedEnumCx`, `enum_identity`.
- Produces:
  ```rust
  // index.rs (field_in_table delegates to it)
  pub fn field_with_owner_in_table(&self, graph: &ProgramGraph, from_object: &ObjectNode, base: &ObjectNodeId, field_lc: &str) -> Option<(FieldNode, ObjectNodeId)>;
  ```

- [ ] **Step 1: Write the failing tests:**
  - `i264_t3_rhs_shapes_complete_at_resolver`: for each routine of the existing `shapes_model` fixture (FromLocal, FromParam, FromField, FromQuotedValue, FromEnumValue, FromInteger, FromOwnCall) `interface_completeness == Complete`. T3 break (spec): drop the `Enum::"K"::V` arm → only `FromEnumValue` fails.
  - `i264_t9b_field_procedure_collision_stays_open`: field `Kind` of the CLOSED enum and procedure `Kind()` returning an EXTENSIBLE enum, `S := Rec.Kind;` → `Partial`. Doc cites alc **AL0486** (compiles with a warning). Break: drop `table_scope_has_routine` (`index.rs:1032`) → `Complete`.
  - `i264_t9c_array_shadowed_call_stays_open`: local `GetKind: array[2] of Enum "Open Kind"` and own procedure `GetKind(i): Enum "Demo Kind"`, `S := GetKind(1);` → `Partial`. Break: drop the shadow guard → `Complete`.
  - `i264_t9d_named_return_shadowed_call_stays_open`: the routine's named return value is `GetKind` → `Partial`. Break: drop only the return-name check → `Complete`.
  - `i264_t29_two_closed_enums_are_complete`: `S := A.Kind;` (enum K1) and `S := B.Kind;` (enum K2), both closed and both implementing the interface → `Complete`, and `closed_enum_sources` (via `SiteFacts`) lists both. Model half in Task 5.
  - `i264_rf1_enum_by_number_and_namespace_closes` (Review Focus 1): `K: Enum 50102;` and a namespace-qualified `Enum "Demo.Demo Kind"` declaration → `Complete`.
  - T21: add to the doc of the existing `i264_t21_same_name_enum_resolves_in_the_declaring_app`: "Defensive, stated precondition: alc rejects this closure (AL0197, enum name declared by two extensions, probe P7); al-sem must still never close it." Break: resolve the field's enum from the caller's object → the test fails.
- [ ] **Step 2: Run** them → the closed-expectation ones FAIL.
- [ ] **Step 3: Implement** `enum_identity`'s remaining arms per spec §3 rule 4: field (bare `Member` only, `!index.table_scope_has_routine(..)`, resolve from the owner id returned by `field_with_owner_in_table`); `Enum::"X"::V`, `"X"::V` (only when no local/param/global named X), `Enum::"X".FromInteger(n)`; bare call (decline on params/locals/globals/named-return name match, then `resolve_bare` must give exactly one `RouteTarget::Routine(id)`, read that routine's return type, resolve from `id.object`). Everything else `None`.
- [ ] **Step 4: Run** → PASS; discrimination as listed.
- [ ] **Step 5: Gate** `scripts/ci-steps task-affected` → 0.
- [ ] **Step 6: Commit** `src/program/resolve/{closed_enum.rs,index.rs} tests/cli/cli_d35_unknown_commit.rs` — "resolve: declaring-context enum identity with field/call binding guards (#264)".

### Task 5: `closed_over` through the model, combined graph, snapshot and prove/digest

Owns **T10, T12, T13, T14, T29 (model half)**, and turns **T1, T2, T3, T4, T22, T27** green; T9's discrimination proof.

**Files:**
- Modify: `src/program/model/calls.rs:19-27`, `src/program/model/program_calls.rs:2101-2150`, `src/program/model/program_calls_adapter_tests.rs:405-410`
- Modify: `src/engine/l4/combined_graph.rs:304-404`
- Modify: `src/engine/l5/snapshot.rs:2171-2215`
- Modify: `src/engine/l5/unresolved_cone.rs:147-149`
- Modify: `CHANGELOG.md` (Fixed: "Interface calls whose value provably comes from a non-extensible source enum are a closed candidate set: no `interface-open-world`, `openWorld: false`, no prove/digest blocker; d35 no longer reports an unknown Commit for them (#264).")
- Test: `tests/cli/cli_d35_unknown_commit.rs`

**Interfaces:**
- Consumes: Task 3 `SiteFacts.interface_closed_over`.
- Produces: `DispatchMeta { …, pub closed_over: Vec<String> }` (sorted model object ids; filled only when `dependency.is_empty()` and `unresolved_impls.is_empty()`, spec §5).

- [ ] **Step 1: Write the failing tests:**
  - T12 `i264_t12_two_candidates_emit_no_open_world`: two values → two codeunits → `assert_closed(&site, &[implA, implB] (sorted stable ids), "T12")`. Break: skip only on the meta-carrying edge → red.
  - T13 `i264_t13_committing_candidate_still_reports_yes`: closed site, `Demo Impl.NextKm` calls `Commit()` → one d35 finding, severity `high`, YES wording (`assert_yes_finding`). Break: suppress the candidate edge in `combined_graph` → no finding.
  - T14 `i264_t14_unmapped_committing_implementer_reports_yes`: a second codeunit implements the interface, is NOT mapped by the enum, and commits → d35 YES (stated limit). Break: filter routes to `enum_impls` → no finding.
  - T29 `i264_t29_two_closed_enums_list_both_in_closed_over`: the Task 4 T29 fixture on the cross-app model → `site.meta` contains `closed_over: [<K1 id>, <K2 id>]` sorted (equal to `enum_implementers` because the fixture has exactly those two enums), `openWorld == Some(false)`.
  - T10: no new test; `cargo test --test cli cli_graph_export::` must stay green (routes unchanged).
- [ ] **Step 2: Run** `cargo test --test cli cli_d35_unknown_commit::i264_` → T1, T2, T3, T4, T12, T22, T27 (cross-app half), T29 FAIL; T13, T14 PASS as controls.
- [ ] **Step 3: Implement** spec §5–§6: adapter fills `closed_over`; `combined_graph::assemble` builds `closed_sites` once from meta-carrying edges under the owning-row rule and skips the targeted-edge uncertainty for them without inserting into `interface_open_world_emitted`; snapshot `open_world: Some(!closed)` from `representative.dispatch_meta`; `unresolved_cone` skips `status == "polymorphic" && open_world == Some(false) && unresolved_candidates` empty. Update the adapter test literal (`closed_over: Vec::new()`). CHANGELOG entry.
- [ ] **Step 4: Run** `cargo test --test cli cli_d35_unknown_commit::` → all PASS (incl. A3/A4/T21 controls); `cargo test --test cli cli_graph_export:: cli_digest_test_coverage::` → PASS. Discrimination: T1 (delete the combined-graph skip), T2 (restore `completeness_for_shape` keeping `closed_over` → its `Complete` assert fails), T22 (drop the `unresolved_cone` skip), T27 (drop `dependency.is_empty()`), T12–T14, T29 (keep only the first enum), T9 (set `closed_enum: true` in `abi_ingest` → T9 wrongly closed).
- [ ] **Step 5: Gate** `scripts/ci-steps task-affected` → 0; then `scripts/check-goldens` (no regen) → green (F4).
- [ ] **Step 6: Commit** the files above — "model/l4/l5: closed_over through adapter, combined graph, snapshot and prove (#264)".

### Task 6: CDO measurement M1–M4 and branch gates

Owns M1–M4, the measured cross-app closed-site count, F1/F2 triage.

**Files:**
- Test: `tests/cli/cli_d35_unknown_commit.rs` (`i264_cdo_closed_site_census`, CDO-gated)
- Modify: `.agent/issue-264/ledger.md` (measurements), spec only if a fallback fires

- [ ] **Step 1: Census test** `i264_cdo_closed_site_census`: `let Some(ws) = crate::cdo::cdo_ws_or_enforce() else { return };` (add `#[path = "../common/cdo.rs"] mod cdo;` to `tests/cli/main.rs` if absent); build `cross_app_model(&ws)`; collect every `compose_snapshot` polymorphic row with `open_world == Some(false)` and its edge's `closed_over`; print one line per site (owning routine, enum ids); assert every listed enum's `ObjectNode.closed_enum` is true; assert the count equals `CDO_CLOSED_SITES` (a const set to the measured value in Step 3, with a comment naming the baseline `bc3ccb18` and the date — a deliberate new pin, re-pinned only with evidence).
- [ ] **Step 2: Build** `cargo build --profile release-fast --bin aldump --bin alsem > /work/bench/264/after-build.log 2>&1`. Before = the master 9bd2996f outputs already in `/work/bench/264/cdo/` (`stats.json`, `switch/`, `an-primary.json`, `an-all.json`); after → `/work/bench/264/cdo-after/` with the same commands (`aldump --switch-dump`, `aldump --program-call-graph-stats`, `alsem analyze <ws> --format json --deterministic --scope primary|all`) on `/work/src/cdo-baseline/Cloud`.
- [ ] **Step 3: M1** `diff <(jq -S . cdo/stats.json) <(jq -S . cdo-after/stats.json)` → empty (else F1, stop). **M2** `aldump --switch-compare cdo/switch cdo-after/switch` → only the `closed_over` field differs, and no single-app site has a non-empty `closed_over` (spec P4a: 0). Run the census test with `CDO_WS=/work/src/cdo-baseline/Cloud cargo test --profile cdo --test cli i264_cdo_closed_site_census -- --nocapture`; record the MEASURED cross-app closed-site count (the 2 `IPrePostValidator` sites may or may not be among them) and spot-check ≥10 sites (or all) against source; set `CDO_CLOSED_SITES`.
- [ ] **Step 4: M3** via the `cdo-measure` skill: per detector, findings added/removed, fingerprints, `confidence`/`cappedBy`, d1 cohort reasons and coverage reasons, primary and all. Triage every moved item against source (F2); record in the ledger. Spec prediction: d35 52/162 unchanged.
- [ ] **Step 5: M4** `scripts/check-goldens` (no regen) → green; `scripts/ci-steps all > /work/bench/264/ci-all.log 2>&1; echo $?` → 0; `bash scripts/cdo-gate > /work/bench/264/cdo-gate.log 2>&1; echo $?` → 0 with no pin edit (the new census pin is the only new CDO assertion). Repro check: `/work/bench/256/repro-{closed,direct-closed}` → no d35; `repro-{ext,direct}` → unchanged medium finding.
- [ ] **Step 6: Commit** `tests/cli/cli_d35_unknown_commit.rs tests/cli/main.rs` — "tests: CDO closed-site census pin (#264)" (the ledger is not committed).

---

## T-number ownership

| Task | Owns |
|---|---|
| 1 | lowering (feeds T15–T17, T32, RF5) |
| 2 | T9, T15, T16, T17, T31, T33, RF4, RF5 |
| 3 | T6 (disc), T8b, T11, T18, T19, T20, T23, T24, T25, T26, T28, T30, T32, T34, RF2, RF3 |
| 4 | T9b, T9c, T9d, T21, T29 (resolver), T3/T2 resolver half, RF1 |
| 5 | T10, T12, T13, T14, T29 (model); T1, T2, T3, T4, T22, T27 green; T9 discrimination |
| 6 | M1–M4, census |
