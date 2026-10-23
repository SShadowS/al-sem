# Issue #240 ledger

## Claim

```json
{"run_id":"20261023-174055-659a27","issue":240,"attempt":1,"branch":"issue/240-agent-discovery-program-model-a1","worktree":"/work/al-sem-issue-240-a1","body_hash":"f752bbc098e4fc80f90c97f271112da0d74a5a35fe61f0e749aada7f716a0add"}
```

- Base: `7c35891a` (master, "analyze: routine-independent finding key (looseFingerprint) (#247) (#251)").
- Body hash: `f752bbc098e4fc80f90c97f271112da0d74a5a35fe61f0e749aada7f716a0add`.

## Fold-in of #249

#249 has the same root cause as #240. `ModelObject.name` keeps the raw IR name, so
`""` stays doubled. #240 sees it in query effects and digest `objectDisplay`. #249
sees it in the identity table, contracts and `alsem fingerprint --format json`.
Conductor decision: this branch delivers both acceptances. The PR says
"Closes #240" and "Closes #249".

## Classification

**Bounded.** Reasons:
- One root: the model copies the IR name without the unescape that the ids
  already apply (#83). The fix is one line at model construction, for objects and
  tables.
- The name is also a lookup KEY in four lookup functions (the symbol table's two
  name lookups, `extension_fields`, and the test-only `Model::table_by_name`). The
  compiler found every read of the field (probe P1), so the key sites are a closed
  list. Spec round 1 changed how the lookups are called (each caller states where
  its name came from); that keeps the work bounded, because the compiler lists
  every caller once the signature changes.
- No golden and no CDO object holds an escaped object name (probes P7, P8). So no
  golden should move.
- None of the hands-off files (`digest.rs`, `ordering_engine.rs`,
  `fingerprint_query.rs`) needs an edit. Their output moves only through the model.

Words used below:
- **raw name**: the name as written inside the quotes, with `""` kept
  (`A ""B"" C`). This is what the IR holds (`ident_text` strips only the outer
  quotes).
- **logical name**: the name AL means, with each `""` turned into `"`
  (`A "B" C`). Ids use it since #83. Dependency (ABI) names are already logical.

## Probes

Line numbers are at base `7c35891a`.

| # | probe | result | evidence |
|---|-------|--------|----------|
| P1 | Every read of `ModelObject.name` and `ModelTable.name` | Found with the compiler: the two fields renamed in a scratch copy, `cargo check --all-targets`. 50 lib sites (plus test-only constructors). Integration tests were grepped by hand. | scratch copy `probe240`, log `probe-check.log`; list in P2/P3 |
| P2 | Where the name is only SHOWN (display) | Identity table (`l5/snapshot.rs:1397`, `:1400`); query `RoutineJoin` / `Owners` / table names (`l4/effect_query_cli.rs:302`, `:398`, `:478`, `:502`, `:621`, `:241`, `:268`, `:272`); HTML report (`gate/format_html.rs:175`, `:185`, `:457`); finding location -> SARIF `logicalLocations`, JSON, terminal, PR summary (`gate/projection.rs:86`); finding text (`d2.rs:441`, `d14.rs:201`, `d22.rs:129`, `:153`, `d33.rs:117`, `d52.rs:107`, `d60.rs:217`, `d64.rs:140`, `:152`, `:178`); table display (`l5/table_display.rs:47`); `PTableRecordTypes.name` (`model/workspace.rs:2372`). Digest `objectDisplay` and fingerprint `objectDisplay` reach the name through `Owners::object_display` (`digest_cli.rs:405`, `:820`, `fingerprint_query.rs:308`). | compiler list; `effect_query_cli.rs:436` `object_display` |
| P3 | Where the name is a KEY | (a) `symbol_table.rs:57` `by_type_name` map, read by `object_by_type_name` (`record_types.rs:215` PageExtension base page, `l4/combined_graph.rs:614` `ObjectRun` by name). (b) `symbol_table.rs:70` `tables_by_name`, read by `table_by_name` (`record_types.rs:113`, `:115`, `:190`). (c) `extension_fields.rs:88` `table_index_by_name` (TableExtension -> base table). (d) `Model::table_by_name` (`workspace.rs:2455`, test-only API). (e) HASH: contract `signatureFingerprint` = sha256(`type|number|name`) (`snapshot_full.rs:659`, `ids.rs:401`), and the table shape hash (`snapshot_full.rs:816`). (f) User selectors: `query touches --table` name match (`effect_query_cli.rs:260`), `Object.Routine` selector (`:302`), owner narrowing on the display (`selector_index.rs:216`). (g) Policy field `object.name` (`gate/policy/predicate_fields.rs:314`) and `capability.resource...` table name (`:346`). (h) `census.rs:248` compares the model name with the graph node name. (i) `d14.rs:120` `ends_with_test_ci(name)`. | compiler list + reads |
| P4 | What the lookups are queried WITH | Raw source text in every source case: record variable `table_name` (`ir_walk` `parse_record_table_name`), `declared_type` via `record_table_name_of` (`record_types.rs:187`), `source_table_name` (`node_extract.rs:588`, `raw`), `extends_target` (lowerer `ident_text`), `ObjectRun` `target_ref`. Logical text for ABI objects (`abi_rows.rs:25`, a dependency page's `source_table_name`). So keys and queries are raw-to-raw for source and logical-to-logical for ABI today. | `receiver.rs:3500` `unquote_identifier` strips outer quotes only; `lower/mod.rs:3418` `ident_text` the same |
| P5 | Which sites already unescape | Only the ids: `ir_walk::ir_object_key` (`ir_walk.rs:2695`), `ir_enclosing_member` (`:2794`), `ObjKey::model_key` (`node.rs:113`). No display or lookup site unescapes. `unescape_al_identifier` is NOT idempotent (`scope.rs:90-99`), so a name must be unescaped exactly once. | grep `unescape_al_identifier` |
| P6 | Real check on base (`cargo build --profile cdo --bin alsem`, fixture: `interface "A ""B"" C"` with `P()`, `table 50100 "T ""Q"" X"`, `codeunit 50101 "C ""Q"" U" implements "A ""B"" C"` whose `P()` inserts into `Record "T ""Q"" X"`) | Every name output keeps `""`, while the ids are logical. The table reference RESOLVES today (raw-to-raw). Selectors match only the raw spelling. | see "P6 output" below |
| P7 | Goldens and fixtures with an escaped object name | None. No `.al` file under `tests/` contains `""` at all; no golden holds `\"\"`. Inline Rust fixtures: `tests/cli/cli_numberless_object_ids.rs:81` (T3/T6), `tests/cli/cli_digest_changed_spans.rs:2245`, `tests/l2_ir/ir_robustness.rs:77` (IR level, not affected). | `grep -rlE '""' --include=*.al tests` -> empty; `grep -rn '\\"\\"' tests/*-goldens tests/goldens` -> empty |
| P8 | CDO baseline objects with an escaped name | None. | `grep -rliE '^\s*(table\|page\|codeunit\|...)\s+[0-9]*\s*"[^"]*""' /work/src/cdo-baseline` -> 0 files |
| P9 | Dependency (ABI) objects | Already logical: `abi_rows.rs:25` copies the SymbolReference name, which has no AL escaping (#83 spec D3). After the fix, source and ABI objects hold the same form. | #83 spec `docs/superpowers/specs/2026-10-15-issue-83-numberless-object-ids-design.md` D3, Limits |
| P10 | Other producers of an object name from the same IR field | R0 identity snapshot (`engine/snapshot.rs:156`, name and its `signatureFingerprint`), L2 projection object name (`program/body/l2_workspace.rs:293`). Both raw. The program graph node name (`node_extract.rs:759`) is raw and is a graph key (`ObjKey::Name`). | grep `o.name.clone()` |
| P11 | Finding fingerprints | Not affected. `fingerprint_of` / `loose_fingerprint_of` hash `objectType/objectKey` (already logical), the routine name and `rootCauseKey` (ids). | `l5/fingerprint.rs:155-200` |
| P12 | `alsem diff` and the contract hash | `diff/passes.rs:150` compares `signatureFingerprint`; a change is `ProcedureSignatureChanged` (Breaking). Diffing a snapshot made before the fix with one made after it flags each escaped-name object once. Cross-app, the dependency side already hashes the logical name, so after the fix source and `.app` agree. | `passes.rs:150-166`, `:325` |
| P13 | Census | `object_fact_census` runs over `rows.workspace` (source objects only) against graph nodes (raw). After the fix it would report a name difference for an escaped name unless it unescapes the graph side once. Nothing asserts the census outside `census.rs`'s own unit tests (`gate/run.rs:330` collects it, `switch_dump.rs:75` dumps it). | `census.rs:248`, `gate/run.rs:330` |
| P8b | CDO: any `""` in any quoted text, and dependency names (opus, spec round 1) | Only JSON string literals (`CDOExtensionApp.Table.al:256` and test codeunits), no identifier reference. No object name in the SymbolReference of any `.app` in `Cloud/.alpackages` contains `"`. So neither the rename nor the lookups can move CDO. | opus spec-r1 review, "Probe evidence" |
| P14 | Can a lookup's caller tell where its name came from? | Yes, from the owning object. `ModelObject` has exactly two production producers (compiler list, P1): `project_ir` sets `source_anchor: Some(..)` always (`workspace.rs:918`), and `abi_rows::dep_object_to_l3` sets `None` (`abi_rows.rs:64`). Source objects (workspace, and dependencies with embedded source, which go through `project_ir`) hold raw reference text; ABI objects hold logical text. | `workspace.rs:918`, `:960`; `abi_rows.rs:25`, `:64` |
| P15 | Do record-variable table names reach the lookups and displays from both sides? | Lookups: yes. `record_types::resolve` runs over every routine (`workspace.rs:2002-2005`), and ABI routines carry record variables whose `table_name` comes from the ABI type text (`abi_rows.rs:151`, logical). Source ones are raw (`ir_walk.rs:2056` `parse_record_table_name`, `TableNo` `:2094-2095`, dataitem source table `:2103`). Displays: no. `table_display.rs:51-58` (tier 2) and `d52.rs:108-113` read the record variable of an OPERATION's routine, and ABI routines have no operations (`abi_rows.rs:199` `record_operations: Vec::new()`). So those two displays only ever see source text. | cited lines |
| P16 | Other raw object names in the same outputs | The uncertainty edge's `interfaceName` comes from the program graph's raw node name (`program_calls.rs:2086-2092`) and is hashed into `edgeId` (`l5/snapshot.rs:1033`). `fingerprint --format json` prints it. | cited lines |
| P17 | Literal hashes for the fixture (for O5) | Logical: `sha256("Interface\|0\|A \"B\" C")` = `e9c79d08c44fa746cf5789db704e01146d67291e7228b0d33f050e449979c616`; `sha256("Codeunit\|50101\|C \"Q\" U")` = `f84fcc77443651f307b410f7ccea4d9d323f27c59f5598871eab95197f9a391b`; table shape `sha256({"name":"T \"Q\" X","number":50100})` = `2a4b0802040e2ccd6d6779e6bd3fa4e7c1f158e6992f194ce03991b395cbea2c`. The raw shape hash `c5147399…` equals the base output (P6), which confirms the input format. | `sha256sum` on the literal strings |

### P6 output (base `7c35891a`)

```
$ alsem digest <ws> --file src/Cu.Codeunit.al --file src/Iface.Interface.al --format json --deterministic --single-app
  "objectDisplay": "Codeunit C \"\"Q\"\" U"
  "objectDisplay": "Interface A \"\"B\"\" C"     stableId …:Interface:@a%20%22b%22%20c#db200ad5…
  "resourceDisplay": "T \"\"Q\"\" X"             resourceId 24024024-…-000000000240/table/50100
$ alsem fingerprint <ws> --format json --deterministic   (payload.identities)
  ["…:Codeunit:50101", "C \"\"Q\"\" U"]
  ["…:Interface:@a%20%22b%22%20c", "A \"\"B\"\" C"]
  ["…:Table:50100", "T \"\"Q\"\" X"]
  contract …:Interface:@a%20%22b%22%20c signatureFingerprint 32465fdc… = sha256("Interface|0|A \"\"B\"\" C")
                                       (logical would be e9c79d08…)
$ alsem query effects <ws> --routine P --format json --deterministic
  payload.routine.routine.objectDisplay = "Codeunit C \"\"Q\"\" U"
  payload.effects[0].tableName         = "T \"\"Q\"\" X"
  payload.effects[0].origin.routine.objectDisplay = "Codeunit C \"\"Q\"\" U"
$ alsem query touches <ws> --table 'T "Q" X'    -> matched no table
$ alsem query touches <ws> --table 'T ""Q"" X'  -> touches T ""Q"" X (…/table/50100)
$ alsem query effects <ws> --routine 'C "Q" U.P'   -> matched no routine
$ alsem query effects <ws> --routine 'C ""Q"" U.P' -> effects of Codeunit C ""Q"" U::P
```

## Design

Revised after spec round 1 (reviews `spec-r1-sol.md`, `spec-r1-opus.md`; register
`findings.json`). The round-1 text is kept in the run dir as `ledger-spec-r1.md`.

### Goal

An object's and a table's name in the model is the logical AL name, the same name
the ids already use. Every output that shows the name (digest, fingerprint, query,
identity table, contracts, HTML, SARIF, finding text) then shows `A "B" C`. Every
lookup that matches a written reference to a name keeps binding the same object,
and each name is unescaped exactly once.

### Non-goals

- The program graph's node names and `ObjKey::Name` stay raw. They are graph keys,
  and the LSP / `aldump` show them (the #83 Limit work).
- Routine, field, variable and member names. Only object and table names move.
- The IR (`al-syntax`) keeps its outer-strip-only contract (`ir_robustness.rs:88`).
- Selectors do not support the escaped spelling. The owner narrowing is not changed
  here (#239 owns it).

### Limits (stated, not fixed)

- **`interfaceName` stays raw.** The uncertainty edge's `interfaceName` comes from
  the graph's raw node name (`program_calls.rs:2086-2092`) and is an input to
  `edgeId` (`l5/snapshot.rs:1033`). One fingerprint document can show `A "B" C` in
  the identities and `A ""B"" C` in an `interfaceName`. Changing it moves `edgeId`.
- **Escaped selectors may still match.** `narrow_to_owner` returns the original
  matches when no owner fits (`selector_index.rs:222-226`). So
  `Codeunit C ""Q"" U::P` can still select a unique `P`. The escaped spelling is not
  supported; it may still match through this owner fallback until #239 removes it.
- **#83's ABI Limit is unchanged.** An ABI name whose logical form holds `""` keeps
  the P3 != P8 id gap described in the #83 spec.
- **The program engine's graph stays raw (CR4 -> #250).** `node_extract`'s
  `ObjectNode.name` / `ObjKey::Name` and `receiver.rs`'s `unquote_identifier` keep the
  raw spelling, so the call resolver still compares raw source names with logical
  ABI names. #250 ("every ObjKey::Name holds the folded LOGICAL name") owns it; this
  branch does not touch the resolver. One edge: `abi_ingest` reads the ABI
  projection's `extends_target_name`, so a QUOTED `TargetObject` holding `""` now
  reaches the graph logical, like the dependency's own object names. None exists in
  CDO (P8b) or in any golden.

### Acceptance matrix

CLI tests go in `tests/cli/cli_object_display.rs` (it runs the real `alsem` binary,
`alsem()` helper). The fixture is the P6 workspace unless a row says otherwise.
Hashes are literal hex from P17, never computed with a production helper.

| Acceptance | Test | Discrimination proof |
|---|---|---|
| #240: query effects show the logical name | T1 `escaped_names_are_logical_in_query_effects`: `alsem query effects --routine P --format json`; `objectDisplay == "Codeunit C \"Q\" U"`, `tableName == "T \"Q\" X"`, and the effect's table id is `<guid>/table/50100`. | Revert the model unescape (step 2) -> `C ""Q"" U`; fails. Restore -> passes. |
| #240: digest `objectDisplay` | T2 `escaped_names_are_logical_in_digest`: `alsem digest --file` both files; `objectDisplay` is `Interface A "B" C` and `Codeunit C "Q" U`; `resourceDisplay == "T \"Q\" X"` with `resourceId` `…/table/50100`. | Same revert -> fails. |
| #249: identity table, fingerprint json | T3 `escaped_names_are_logical_in_fingerprint_identities`: `alsem fingerprint --format json`; identities hold `A "B" C`, `C "Q" U`, `T "Q" X` for their ids. | Same revert -> fails. |
| #249: contracts; table shape (S2/O3, O5) | T4 (same run as T3): the Interface contract's `signatureFingerprint` is the literal `e9c79d08c44fa746cf5789db704e01146d67291e7228b0d33f050e449979c616`, the Codeunit's `f84fcc77443651f307b410f7ccea4d9d323f27c59f5598871eab95197f9a391b`, and the table `schemaFacts` `shapeFingerprint` is `2a4b0802040e2ccd6d6779e6bd3fa4e7c1f158e6992f194ce03991b395cbea2c`. | Same revert -> the raw hashes (`32465fdc…`, `936e2be9…`, `c5147399…`); fails. |
| #249: T6's display-name assertion | Edit `cli_numberless_object_ids.rs:172-174` to `"Ø \"Q\" /:#%@ If"` with its comment; edit `cli_digest_changed_spans.rs:2254` to `find(o.name == "I\"Foo")` ("the model holds the logical name"). | Revert -> both fail. |
| R0 and L2 agree with the model (S5/O2) | T5 in-process test (new member of the `cli` umbrella, or `tests/differential.rs` if it fits better): `engine::snapshot::snapshot_workspace` and `program::body::l2_workspace::project_workspace` on the P6 fixture. R0's interface name is `A "B" C` and its `signatureFingerprint` is the literal `e9c79d08…` (the same literal T4 asserts for the model contract). L2's object name is `A "B" C`. | Revert the R0 producer alone -> R0 name and hash fail. Revert the L2 producer alone -> the L2 name fails. Each restored -> passes. |
| Key use: colliding written names (S1) | T6 model-level (`assemble_and_resolve_workspace_program`): tables `50110 "A""B"` (logical `A"B`) and `50111 "A""""B"` (logical `A""B`), both declared; one codeunit with `R1: Record "A""B"` and `R2: Record "A""""B"`, each doing `Insert()`. R1's op binds `…/table/50110`, R2's `…/table/50111`. Plus a `Logical` guard: a unit assertion `table_by_name(Logical("A\"\"B"))` (the logical name of 50111) returns 50111, and a trigger of table 50111 doing `Rec.Insert()` (implicit `Rec`, resolved through the `obj.name` re-lookup, `Logical`) binds `…/table/50111` (S1, round 2). | Break: treat every name as `Logical` (no unescape) -> R1 binds 50111 or nothing; fails. Break: unescape `Logical` names too -> the `Logical("A\"\"B")` lookup returns 50110 and the implicit-`Rec` op binds 50110; fails. |
| Key use: every raw reference shape (O1) | T7 model-level, one fixture with the escaped table `T ""Q"" X`: a `TableExtension` extending it (its fields merge onto the base table), a `Page` with `SourceTable = "T ""Q"" X"` and a trigger doing `Rec.Insert()`, a `PageExtension` of that page doing `Rec.Insert()`, a codeunit with `TableNo = "T ""Q"" X"` whose `OnRun` does `Rec.Insert()`, a pass-2b variable (one not in `record_variables`, if it can be built; otherwise the test says pass 2b shares pass 1's input), and `Codeunit.Run(Codeunit::"C ""Q"" U")` (the `ObjectRun` target object id). | Break the `Raw` arm (no unescape) -> each assertion must fail. F5: with the break applied, check EACH assertion; one that stays green is resolved elsewhere (the program engine) and the test says so instead of claiming a guard. |
| Key use: cross-app (O6) | T8 CLI, after `cli_digest_query_dependencies.rs::symbol_only_dependency_table_resolves_and_is_selectable`: a symbol-only dependency declares table `T "Q" X` (logical in SymbolReference); the workspace writes `Record "T ""Q"" X"` and inserts. The effect binds the dependency table id. This misses on base (raw vs logical). | Break the `Raw` arm (no unescape) -> unknown table; fails. Restore -> passes. |
| Source-text displays (S4) | T9: an unloaded table, `R: Record "Missing ""Z"" T"` (no such table), with `R.DeleteAll()`. `describe_table` tier 2 shows `Missing "Z" T (type not loaded)`, and d52's fallback text shows `Missing "Z" T`. Prefer a production path (an `alsem analyze` finding that prints it); if no default detector fires on a small fixture, a `table_display` unit test plus a stated limit. | Revert the display unescape -> `Missing ""Z"" T`; fails. |
| Census (O8) | T10 `census.rs` unit test on the P6 shape: `object_fact_census` has no `name` line. | Drop the graph-side unescape -> a `name` line; fails. |

Every proof is made with Edit, run, reverted with Edit, and recorded with both
outcomes (CLAUDE.md).

### Design

1. **One helper.** `ir_walk::ir_object_name(o: &ObjectDecl) -> String`
   (`unescape_al_identifier(&o.name)`), next to `ir_object_key`.
2. **Model (the root).** `model/workspace.rs:800`: `let name = ir_object_name(o);`.
   This sets `ModelObject.name` and, through `index_table_ir(.., &name, ..)`,
   `ModelTable.name`. Every display in P2 then shows the logical name with no edit,
   including the hands-off files.
3. **Same-object producers.** `engine/snapshot.rs:156`: REPLACE
   `strip_quotes(&obj.name)` with `ir_object_name(obj)` (not wrapped: the IR name is
   already outer-stripped, and `strip_quotes` after an unescape would eat a name
   like `""X""`). Remove the "al-sem parity" comment; that reason is retired.
   `program/body/l2_workspace.rs:293`: `let name = ir_object_name(o);`.
4. **Normalize at production (final round 1, CR3; replaces the round-1 text below).**
   Decided in final round 1 (CR3) over the panel-approved typed lookups, because the
   model's reference fields are produced in two closed constructors (`project_ir` /
   `ir_walk` for source, the ABI projection / `abi_rows` for dependencies). Every name
   the model stores is the logical name, unescaped exactly once where it is produced;
   the four lookups take logical `&str` and never unescape. Producers:
   - source: the model's `source_table_name` and `extends_target_name`
     (`workspace.rs`); `ir_walk::parse_record_table_name` (record variables, event
     parameters, L2 `PRecordVariable.tableName`); `TableNo` (exactly one outer quote
     pair, CR1); the report dataitem table; the `Codeunit.Run` target name;
   - `record_types::record_table_name_of` (pass 2b, and ABI parameter types: the
     AlreadyQuoted shape is source text; the Full shape holds no inner `"`; F1/CR6);
   - ABI: `symbol_reference::unquote_abi_name` unescapes only a quoted
     (source-shaped) `SourceTable` / `TargetObject`; a bare one is already logical.
   - Displays (`table_display` tier 2, d52) read the stored logical name (no own
     unescape). The census still unescapes the GRAPH side (name, `extends`, source
     table), because the graph stays raw (#250).
   - `NameRef`, `name_origin()` and the fn-pointer dispatch are removed (CR2, CR8,
     CR9, G1). CR7 (a `Raw` default for an ownerless routine) is moot: no lookup has
     a provenance any more. `ir_object_name` is `pub(crate)` (CR10).

   *Superseded round-1 text, kept for the record:* **Provenance-typed lookups.** In `model/symbol_table.rs` add
   `pub enum NameRef<'a> { Raw(&'a str), Logical(&'a str) }` with one method that
   returns the logical name: `Raw` is unescaped exactly once, `Logical` is used as is.
   The four lookups take a `NameRef` instead of `&str`:
   `SymbolTable::object_by_type_name`, `SymbolTable::table_by_name`,
   `extension_fields::table_index_by_name`, `Model::table_by_name`. One enum is
   smaller than eight named functions, and the signature change makes the compiler
   list every caller. Each caller states its provenance:

   | Caller | Name | Provenance |
   |---|---|---|
   | `record_types.rs:113` (number -> table object -> name) | `obj.name` | `Logical` (model name) |
   | pass 1, `record_types.rs:137` | `variable.table_name` (record vars, `TableNo` `ir_walk.rs:2094`, dataitem source table) | owning object's |
   | pass 2b, `record_types.rs:187-190` | `record_table_name_of(declared_type)` | owning object's |
   | pass 3 `Table`, `:205` | `obj.name` | `Logical` |
   | pass 3 `Page`, `:206-209` | `source_table_name` | the page's |
   | pass 3 `TableExtension`, `:210-213` | `extends_target_name` | the extension's |
   | pass 3 `PageExtension`, `:214-217` | `extends_target_name`, then the base page's `source_table_name` | the extension's, then the base page's |
   | `combined_graph.rs:614` | `ObjectRun` `target_ref` | `Raw` (it comes from a routine body; ABI routines have none) |
   | `extension_fields.rs:46` | `extends_target_name` | the extension's |
   | `Model::table_by_name` callers (tests) | literal names | `Logical` |

   "Owning object's" provenance is `ModelObject::name_origin()`, a new method:
   `Raw` when `source_anchor.is_some()` (built by `project_ir` from source text),
   `Logical` when it is `None` (built from ABI). P14 shows these are the only two
   producers. A routine with no owning object defaults to `Raw` (source), as the
   conductor's rule says. `resolve_table_ref_to_id` forwards the `NameRef` it is
   given; a number string is unaffected.
5. **Source-text displays.** `table_display.rs:51-58` (tier 2) and `d52.rs:108-113`
   unescape the record variable's `table_name` once for display. Both read the
   variable of an operation's routine, and only routines with a body have
   operations (P15), so the text is always source text. A one-line comment says so.
6. **Census.** `census.rs:248` compares `o.name` with
   `unescape_al_identifier(&n.name)`. The census reads workspace objects only, so the
   graph name is raw and is unescaped once. `extends`/`sourceTable` stay raw on both
   sides.
7. **Not changed.** `extends_target_name`, `source_table_name`,
   `implements_interfaces` and record-variable `table_name` keep the text as
   written; step 4 converts them at the lookup. The program graph stays raw.
8. **Docs.** Update the caller list in `unescape_al_identifier`'s doc
   (`scope.rs:90-97`): add `ir_object_name`, `NameRef`'s `Raw` arm, the two displays
   and the census, and keep the "exactly once" warning.
9. **CHANGELOG** (Fixed):
   - object and table names are shown unescaped in every output;
   - the contract `signatureFingerprint` of an object whose name holds `""`, and the
     table `schemaFacts.shapeFingerprint` of such a table, change once. `alsem diff`
     reports the contract change as `ProcedureSignatureChanged`; its schema pass compares
     shape fingerprints only for fields and enum values, not for tables
     (`passes.rs:423-470`, `:522`);
   - a source reference written with `""` to a dependency object now resolves
     (it missed before: raw vs logical), so effects and findings on it can appear;
   - selectors and policy `object.name` match the logical name; the escaped
     spelling is not supported.

### False-positive and failure shapes

- **Double unescape.** Only `ir_object_name`, the `Raw` arm, the two source
  displays and the census unescape, and each takes text that is raw by
  construction. Logical names (model names, ABI text) go through `Logical`.
- **Misbinding (S1).** A raw reference is always unescaped before it is matched,
  so `"A""B"` can never match a table whose logical name is `A""B`. T6 pins it.
- **Lookup miss.** A missed reference sends an effect to the unknown-table bucket.
  T1/T2 assert the real table id; T7 and T8 cover every raw shape and the
  cross-app case.
- **Wrong provenance marker.** If a future producer builds a `ModelObject` from
  source without an anchor, its references would be read as logical. The
  `name_origin()` doc names the rule; T7/T8 fail if the marker is wrong for
  today's two producers.
- **Ids.** None change: `object_key` already unescapes (#83); table ids carry no
  name.
- **Fingerprints.** Finding fingerprints and `looseFingerprint` do not hash the
  name (P11). Contract `signatureFingerprint`, R0 `signatureFingerprint` and table
  `shapeFingerprint` do, and change once for an escaped name (P12, step 9).
- **Sorting.** The identity table is keyed by id; query hits sort by id. Only
  display-sorted outputs (HTML) may reorder, and only escaped-name rows.
- **Selectors and policy.** Logical names match. The escaped spelling is not
  supported but may still match through the owner fallback (Limits).
- **#83 T6.** Its display assertion becomes the logical name (#249's acceptance).
  T3's id assertions do not change.

### Measurement plan

- `scripts/check-goldens`: expect NO golden to move (P7). A move outside a display
  name or a name hash stops the work (F1).
- `scripts/ci-steps task`, then the branch gate `scripts/ci-steps all` and
  `bash scripts/cdo-gate`. CDO has no escaped name anywhere (P8, P8b), so the
  unknown count, findings and fingerprints must not move. A move means a lookup
  changed for a name without `""`, which is a bug.
- Re-run the P6 commands on the branch build; record the new output beside the
  base output.

### Fallbacks (stated in advance)

- **F1.** If a golden moves in a field that is not a display name or a name hash,
  stop and find why before any regen.
- **F2.** If T5 cannot be made to fail for a producer, or step 3 moves anything
  unexpected, drop that producer's change and state the R0/model hash gap as a
  Limit.
- **F3.** If a call site's provenance cannot be determined, record it here and
  default it to `Raw` (source).
- **F4.** If `source_anchor` proves an unreliable marker (a source object without
  one, or an ABI object with one), add an explicit `origin` field to `ModelObject`
  set by its two producers, and use it in `name_origin()`.
- **F5.** If a T7 assertion cannot fail under its break (the program engine
  resolved that reference first), the test states that limit, and the CLI-level
  table-id asserts remain the guard.
- **F6.** If no default detector prints the tier-2 or d52 fallback on a small
  fixture (T9), use a `table_display` unit test and state that limit.

## Timeline (conductor)

- Spec round 1 (sol + opus, fresh): sol S1-S5, do not approve; opus O1-O8, approve with O1/O2 as conditions. Reviewers split on D4 (sol: exact-first misbinds a colliding pair; opus: always-unescape double-decodes logical inputs). Conductor decision: provenance-typed lookups (`NameRef { Raw, Logical }`), compiler-enumerated callers. Design revised by the design author (opus), 13/13 fixed.
- Spec round 2: opus accepted all 13, raised O9 (important); sol accepted 12, re-raised S1, the same point: T6's Logical-arm break could not fail. Both confirmed `source_anchor` is a reliable provenance marker and the caller table is complete. Conductor added the Logical guard to T6 (L141).
- Spec round 3: both accepted S1 and O9; both approve. Register: 14 entries, all accepted by both. spec_rounds remaining 2.
- Hands-off (alsem-main, #160 step 6): digest.rs, ordering_engine.rs, fingerprint_query.rs.

## Acceptance tests: red on base (`7c35891a`)

Run: `cargo test --test cli -- escaped cli_numberless_object_ids::full_snapshot_keeps quoted_interface_name_is_unescaped`
(the T6 `Logical` guard was taken out for this run: `NameRef` did not exist yet, so its red is
the missing API). 9 failed, 4 passed.

| Test | Red on base |
|---|---|
| T1 query effects | `objectDisplay` left `Codeunit C ""Q"" U`, right `Codeunit C "Q" U` |
| T2 digest | owners `["Codeunit C ""Q"" U", "Interface A ""B"" C"]` |
| T3 identities | `C ""Q"" U` vs `C "Q" U` |
| T4 hashes | Interface contract `32465fdc…` vs `e9c79d08…` |
| T5 R0/L2 | R0 `("A ""B"" C", "32465fdc…")` vs `("A "B" C", "e9c79d08…")` |
| #83 T6 edits | identity table `Ø ""Q"" /:#%@ If`; `find(o.name == "I\"Foo")` -> None |
| T8 cross-app | `[("DB_INSERT", "", "")]` (no table) vs `…02de/table/18`, `T "Q" X` |
| T9 displays | `Insert on Missing  (type not loaded)` and `(Missing )` (see "Deviation" below) |
| T6 (no Logical guard), T7 | GREEN on base, as expected: raw-to-raw matching binds today. They pin that the fix keeps binding; their proofs are the breaks below. |
| T10 census | green on base (raw vs raw); it guards the census change, proof P8. |

## Implementation

### Commits

- `7cdf6501` model: object and table names are logical; lookups state their spelling (#240, #249)
- `4af9a579` docs: CHANGELOG and plan for logical object names (#240, #249)

Plan: `docs/superpowers/plans/2026-10-23-issue-240-logical-object-names.md`, 2 tasks.
`check-diff --issue 240` against `origin/master` (= `7c35891a`, not moved): `reasons: []`
before each commit and after the last.

### Deviation from the spec (not a pre-stated fallback)

**`ir_walk::parse_record_table_name` truncated a quoted name at its first inner quote**:
`Record "Missing ""Z"" T"` gave `table_name = "Missing "`. P4/P15 assumed the record
variable's `table_name` holds the raw text; for an escaped name it did not. Effects:
T9's displays printed `Missing ` (no step-5 unescape could fix that), and pass 1 of
`record_types` missed every escaped record variable, which pass 2b
(`record_table_name_of`, which strips the outer quotes properly) then caught. Fix: the
parser skips a doubled `""` and returns the whole raw inner text (design step 7: "keep
the text as written"). Pre-existing bug in the same area; no fallback F1-F6 covers it.
It cannot move CDO or a golden (P7, P8, P8b: no `""` anywhere). It sits in commit
`7cdf6501`; dropping it means T9 must be rewritten as a limit. Flagged for the conductor.

### Gate and goldens

- `bash scripts/ci-steps task`: exit 0 (3853 tests passed, 8 skipped).
- `scripts/check-goldens` (no regen): exit 0, 1010 passed. No golden moved, as the design
  expected. `--regen` was not run: the regen hook demands a triage receipt for moved
  goldens, and a byte-compare pass already proves none moved.
- Branch gate (`ci-steps all`, `cdo-gate`) not run here; it is the merge gate.

### P6 re-run on the branch build

```
query effects --routine P        -> objectDisplay "Codeunit C \"Q\" U", tableName "T \"Q\" X", tableId …/table/50100
fingerprint identities           -> ["C \"Q\" U", "P", "A \"B\" C", "T \"Q\" X"]
query touches --table 'T "Q" X'  -> name "T \"Q\" X", id …/table/50100   (base: matched no table)
query effects --routine 'C "Q" U.P' -> "Codeunit C \"Q\" U"              (base: matched no routine)
```

### Discrimination proofs

Each break made with Edit, applied-check by `grep -c` of the break text (1 = applied; where
rustfmt reflowed or the break equalled the base text, the failing output itself shows it
applied), run, reverted with Edit. All reverted: the final tree is the committed one, and the
gate ran after the last revert. Pass column = the green run after revert (gate run).

| # | Test(s) | Break | Fail output (under the break) | Pass | Commit |
|---|---|---|---|---|---|
| P1 | T1, T2, T3, T4, #83 T6 (both edits), T5 model line | `workspace.rs`: `let name = o.name.clone();` | T1 `Codeunit C ""Q"" U`; T2 `["Codeunit C ""Q"" U","Interface A ""B"" C"]`; T3 `C ""Q"" U`; T4 `32465fdc…`; numberless `Ø ""Q""…`; changed_spans `find` -> None; T5 `model: ["A ""B"" C"]`. (T6/T7 fail too: raw model names against unescaped Raw refs.) | yes | 7cdf6501 |
| P2 | T5 R0 | `engine/snapshot.rs`: `let name = obj.name.clone();` | `R0 left [("A ""B"" C", "32465fdc…")]` | yes | 7cdf6501 |
| P3 | T5 L2 | `l2_workspace.rs`: `let name = o.name.clone();` | `L2 left ["A ""B"" C"]` | yes | 7cdf6501 |
| P4 | T6 (R1/R2), T7 (all 5 shapes), T8 | `NameRef::Raw(s) => Cow::Borrowed(s)` | T6 `R1 -> …/table/50111, R2 -> None`; T7 `extends merged false, Page None, PageExtension None, TableNo None, Codeunit.Run target None`; T8 `[("DB_INSERT","","")]` | yes | 7cdf6501 |
| P5 | T6 Logical guard (both) | `NameRef::Logical(s)` also unescaped | `Logical lookup -> …:Table:50110`, `implicit Rec of table 50111 -> …/table/50110` | yes | 7cdf6501 |
| P6 | T9 d1 | `table_display.rs` tier 2: `let table_name = table_name.clone();` | `Insert on Missing ""Z"" T (type not loaded).` | yes | 7cdf6501 |
| P7 | T9 d52 | `d52.rs`: `.map(str::to_string)` | `var record parameter R (Missing ""Z"" T)` | yes | 7cdf6501 |
| P8 | T10 | `census.rs`: `n.name.to_string()` | three `name\t…\tmodel=C "Q" U\tprogram=C ""Q"" U` lines | yes | 7cdf6501 |
| P9 | T9 (parser) | `parse_record_table_name`: `if false && bytes.get(i + 1) …` | `Insert on Missing  (type not loaded)`, `(Missing )`; every other test green (pass 2b resolves) | yes | 7cdf6501 |
| P10 | T6, T7, T8 (`name_origin`) | `name_origin`: `if false && self.source_anchor.is_some()` (always Logical) | T6 `R1 -> 50111, R2 None`; T7 four table shapes fail, `Codeunit.Run target` stays green (it is `Raw` directly, not through `name_origin`); T8 no table | yes | 7cdf6501 |

**F5 (T7 under its `Raw` break, P4):** every one of the five assertions failed; none was
resolved elsewhere by the program engine. T7 and T6 were rewritten to collect every value
and compare once, so a single failure shows each assertion's value.

**Stated limits of the proofs:**
- `name_origin` always `Raw` is not pinned: no test has a dependency (ABI) object whose
  written reference holds `""` in logical text. An ABI name with a literal `"` would need
  `""` in it to change under a Raw unescape; none exists in P8b.
- T7's pass 2b has no assertion of its own: once the parser fix lets pass 1 resolve, no
  source shape reaches pass 2b with an escaped name (stated in the test doc).

### Fallbacks fired

None of F1-F6. F5 did not fire (all T7 assertions failed under the break). F6 did not fire:
d1 and d52 are default and print both displays on a small fixture, so T9 runs the real
`alsem analyze`. One deviation outside the fallbacks (the parser fix, above).

## Implementation, final round 1 rework (normalize at production)

Commit `05d24133` model: normalize names at production instead of typed lookups (#240, #249),
on top of `4af9a579`. `check-diff --issue 240 --base origin/master` (origin/master = `7c35891a`):
`reasons: []`. Gate `bash scripts/ci-steps task`: exit 0 (3855 passed, 8 skipped).
`CHECK_GOLDENS_FORCE=1 scripts/check-goldens`: exit 0 (1011 passed). **No golden moved**
(the event `tableName` and L2 `PRecordVariable.tableName` are logical now; no golden holds `""`).

Findings handled: CR3 (root cause, design D4 replaced), F1/CR6 (ABI already-quoted parameter
type, `record_table_name_of`; quoted ABI `SourceTable`/`TargetObject`, `unquote_abi_name`), CR1
(`TableNo` strips exactly one quote pair), CR2/CR8/CR9/G1 (`NameRef`, `name_origin` and the
fn-pointer dispatch removed; the four lookups take logical `&str`, files back to base), CR5
(event and L2 record-variable `tableName` logical, through `parse_record_table_name`), CR7 (moot:
no provenance at lookup), CR10 (`ir_object_name` is `pub(crate)`), G2 (CHANGELOG names the R0
`signatureFingerprint`), CR4 (Limit -> #250, resolver untouched). Not normalized: nothing; every
listed producer was normalized at production. The census keeps a graph-side unescape (name,
`extends`, source table) because the graph stays raw (#250); the conductor's "census drops its
unescape" would make it report every escaped object.

New tests: T7 gains a trailing-escape table `"Q ""X"""` (TableNo, record variable, Page
SourceTable) and a report dataitem shape; T8b
`already_quoted_dependency_parameter_binds_its_logical_table` (cross-app model, symbol-only
dependency with tables `A"B` / `A""B` and a parameter typed `Record "A""B"`); unit tests
`record_types::record_table_name_of_basics` (+2 asserts) and
`symbol_reference::quoted_source_table_and_target_object_are_logical`; T10 gains a
TableExtension and a Page so `extends` / source table are compared.

### Proof table (round 2)

Each break: Edit, `grep -c` of the break text = 1 (applied), run, Edit back. All reverted; the
gate ran after the last revert. Pass = green in the gate run. Commit `05d24133`.

| # | Break (drop the unescape at producer X) | Failing test(s) and output | Stayed green under the break (why) |
|---|---|---|---|
| N1 | model name (`workspace.rs`: `o.name.clone()`) | T1 `Codeunit C ""Q"" U`; T2; T3 `C ""Q"" U`; T4 `32465fdc…`; T5 model; #83 identity `Ø ""Q""…`; changed_spans `find` None; T6, T7 (references now logical against raw names) | T8, T8b, T9 (no escaped workspace object they depend on) |
| N2 | R0 name (`snapshot.rs`: `obj.name.clone()`) | T5 `R0 left [("A ""B"" C","32465fdc…")]` | - |
| N3 | L2 name (`l2_workspace.rs`: `o.name.clone()`) | T5 `L2 left ["A ""B"" C"]` | - |
| N4 | model `SourceTable` (`.map(|s| s.to_string())`) | T7 `Page SourceTable None`, `PageExtension base page None`, `trailing escape: Page SourceTable None` | other T7 shapes (not this producer) |
| N5 | model `extends` (`.map(str::to_string)`) | T7 `extends merged false`, `PageExtension base page None` | - |
| N6 | `parse_record_table_name` (`after[..i].to_string()`) | T6 `R1 -> 50111` (pass 1 matched raw `A""B` against 50111's logical name); T9 d1 `Missing ""Z"" T (type not loaded)` and d52 `(Missing ""Z"" T)` | T7 trailing record var, T8: pass 1 misses, pass 2b (`record_table_name_of`) resolves them. N6+N10 together: T8 `[("DB_INSERT","","")]`, T7 `trailing escape: record variable None`, T1/T2 fail too |
| N7a | `TableNo` unescape (one pair still stripped) | T7 `Codeunit TableNo None`, `trailing escape: TableNo None` | - |
| N7b | `TableNo` back to `trim_matches('"')` + unescape (CR1) | T7 `trailing escape: TableNo None`, every other shape green | - |
| N8 | report dataitem table (`.map(str::to_string)`) | T7 `Report dataitem table None` | - |
| N9 | `Codeunit.Run` target (`strip_quote_chars(tn)`) | T7 `Codeunit.Run target None` | - |
| N10 | `record_table_name_of` (`strip_quotes(rest).trim().to_string()`) | T8b `[(Some("A""B"), …/table/50111)]` (sol's F1 misbind); unit `record_table_name_of_basics` `Some("A""B")` | T8 alone (pass 1 resolves it); see N6+N10 |
| N11 | ABI `unquote_abi_name` (return `inner`) | unit `quoted_source_table_and_target_object_are_logical`: `[Some("A""B"), Some("A"B"), Some("A""B")]` (the bare P2 is the unchanged control) | - |
| N12 | census graph side: name, `extends`, source-table ref (all three dropped) | T10: `name` lines for Codeunit/Interface/Table, `extends` for the TableExtension, `source_table` for the Page | - |
| N13 | lookups unescape their (logical) input (`SymbolTable::table_by_name`, `Model::table_by_name`) | T6 `R1, R2, logical lookup, implicit Rec` all -> 50110 | - |

F5: every T7 shape fails under its own producer's break; none is resolved by the program
engine. The only cross-path rescue is pass 2b for a record variable (N6), recorded above and
pinned by N10 and N6+N10.

Fallbacks fired: none (every producer could be normalized at production).

### Final round 2 (sol F2 = opus G3, opus G4)

Commit `afeea4cb` on `05d24133`. The dataitem-NAMED record variable of a Report /
ReportExtension / XmlPort (`ir_walk.rs` dataitem seeding) stored `di_table` raw; it is now
unescaped once at production. T7 asserts that variable's `table_name` (`T "Q" X`) and
`table_id` (`…/table/50100`); the body walk records no op on a dataitem receiver, so the
variable itself is the assertion. Other `ObjectDecl.dataitems` readers (`node_extract`,
`resolve/full.rs`, `receiver.rs`, `impact_graph/facts.rs`, `lsp/def_surface.rs`) feed the
program graph or the LSP hash, which stay raw (#250). G4: the docs (`record_types.rs`,
`scope.rs`) now claim logical only for the names and references the lookups match, and name
`implements_interfaces` and `ModelPageControl.target` as raw (Limit; no lookup reads them).
Gate `ci-steps task` exit 0 (3855 passed); `CHECK_GOLDENS_FORCE=1 check-goldens` exit 0 (1011
passed, no golden moved); check-diff `reasons: []`.

| # | Break | Failing test and output | Commit |
|---|---|---|---|
| N8b | dataitem-named variable: `table_name: Some(di_table.clone())` | T7: `left Some((Some("T ""Q"" X"), None))`, right `Some((Some("T "Q" X"), Some(…/table/50100)))` | afeea4cb |

## Final panel (conductor)

- Final round 1 (head 4af9a579): opus ship (G1, G2 minor); sol do not ship (F1, ABI already-quoted parameter type misbinds); built-in code-review (high) CR1-CR10, led by CR3 (the model mixed spellings). Conductor decision: normalize at production instead of the spec panel's typed lookups (D4 rewritten; superseded text kept). Rework 05d24133.
- Final round 2 (head 05d24133): both accepted F1, G1, G2, CR1-CR10 (except sol re-raised CR3/CR5), S1, O1, S4; both found the same missed producer (sol F2 = opus G3: named dataitem record variables), opus G4 (doc too broad). Fixed in afeea4cb.
- Final round 3 (head afeea4cb): both accepted F2, G3, G4, CR3, CR5; both ship. Register: 30 entries, all accepted by both, none open. final_rounds remaining 2.
- Step 11 gates on afeea4cb (base 7c35891a, supervised, run 20261023-174055-659a27): check-goldens-coverage exit 0 (0.0 s); ci-steps-all exit 0 (304.6 s); check-goldens exit 0 (0.1 s, tree-stamp skip); cdo-gate exit 0 (276.2 s).
