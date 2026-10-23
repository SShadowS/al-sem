# Issue #253 ledger

## Claim

```json
{"run_id":"20261023-192451-fe0173","issue":253,"attempt":1,"branch":"issue/253-analyze-per-test-procedure-qual-a1","worktree":"/work/al-sem-issue-253-a1","body_hash":"6d7f8a7e1d7e4fa020dbada0cc072a2fdc80bb91712cf6f3d9fb0387904f237f"}
```

- base: `60434aef`
- body_hash: `6d7f8a7e1d7e4fa020dbada0cc072a2fdc80bb91712cf6f3d9fb0387904f237f`
- operator: CentralGauge demo 2026-10-27; release v2.0.3 around 2026-10-25. Keep it bounded.
- hands off: `src/engine/l5/digest.rs`, `ordering_engine.rs`, `fingerprint_query.rs` (#160) and `digest --diff` (#254). This design touches none of them.

## Classification: bounded

Reason: the change is additive and lives in one place. `al-call-hierarchy --analyze`
already walks every routine's IR body in `src/analysis.rs::file_metrics` (it computes
complexity there). The new block is one more syntactic walk over the same IR, plus
two sums. No engine, resolver, golden family or detector changes. The reachable count
(round 1, operator answer 3) reads the call graph #159 already builds for
`--format json`; it adds no resolution. The only existing test it touches is the #159
"legacy fields unchanged" test, which gets two edits (its frozen files stay frozen).

## Timeline

- 2026-10-23 19:25 claim, worktree at base `60434aef`.
- 2026-10-23 19:30 classified bounded; probes P1-P16 run; `## Design` v1 written.
- 2026-10-23 spec panel round 1: sol S1-S6 (3 important), opus O1-O9 (2 important); both "do not approve". Operator answers: snake_case; add `object_stable_id`; add `assert_count_reachable`. Every finding checked against the code (P17-P22); none refuted. `## Design` v2 written; register `findings.json` created.
- 2026-10-23 spec panel round 2: opus approves with minor O10-O12; sol does not approve (re-raises S2, S3, O2; new S7-S11). Every finding checked (P23-P26); none refuted. Conductor decisions applied. `## Design` v3 written; register updated (S2, S3, O2 re-anchored; S7-S11, O10-O12 appended). Pre-edit copy: run dir `ledger-spec-r2.md`.

## Probes

| probe | result | evidence |
|-------|--------|----------|
| P1 where `--analyze --format json` is built | `src/main.rs::run_analysis` calls `analysis::analyze_project` and prints `serde_json::to_string_pretty(&AnalysisResult)`. `AnalysisResult` = `{metrics, findings, summary}`. No other caller of `analyze_project` or `AnalysisResult` (grep over `src tests benches`). | `src/main.rs:186-221`, `src/analysis.rs:66-71,255-325` |
| P2 is the IR reachable per routine | Yes, the full IR. `file_metrics` holds `file: Arc<AlFile>` (the snapshot's own parse, or a fresh `al_syntax::parse`) and loops `for obj in &file.objects { for r in &obj.routines }`. It already walks `r.body` with `routine_complexity_ir(&file.ir, r)`. So attributes (`r.attributes`), locals (`r.locals`), params (`r.params`), object globals (`obj.globals`) and the body are all at hand. The IR walk does not need the call graph, so it works for text/CSV too and without `app.json`. | `src/analysis.rs:344-458` |
| P3 #159 history | `809fe684` (#159) moved `analyze_project` into `analysis.rs` and appended `path`, `object_id`, `access`, `is_event_subscriber`, `is_trigger`, `routine_id`, `fan_in`, `fan_out` to each metrics row. Last touch `60434aef` (#240/#249, names). | `git log --oneline -6 -- src/analysis.rs` |
| P4 `[Test]` in the IR | `RoutineDecl.attributes: Vec<String>`, names lowercased (`"test"`, `"handlerfunctions"`, ...). `analysis.rs::is_framework_invocation_attribute` already matches `"test"` this way. | `crates/al-syntax/src/ir/decl.rs:183-190`, `src/analysis.rs:642-659` |
| P5 `[Test]` inside an `#if` arm | Routines from every textually present `#if` arm are in `obj.routines` (each carries `preproc_context`). A test written in both arms is two declarations, so two rows. | `crates/al-syntax/src/ir/decl.rs:5,225-230` |
| P6 `asserterror` | A statement kind: `StmtKind::AssertError(BlockId)`. Confirmed on a probe: `asserterror Error('x')` lowers to it and the call inside carries `underAsserterror: true`. | `crates/al-syntax/src/ir/stmt.rs:87`; probe ws `scratchpad/probe253`, `aldump --l2` |
| P7 how an assert call looks | `Assert.AreEqual(a, b, m)` is `ExprKind::Call { function: Member { object: Identifier("Assert"), member: "AreEqual" }, args }`. `aldump --l2` shows `callee {kind: member, receiver: "Assert", method: "AreEqual"}`. | `crates/al-syntax/src/ir/expr.rs:13-74`; probe output |
| P8 the receiver's type | `VarDecl.ty: Option<String>` is the raw type text (`Codeunit "Library Assert"`). `program::resolve::receiver::classify_type_text` (pub) turns it into `ParsedType::Object { kind: Codeunit, object_ref: Name{normalized_lc} | Id(n) }`, stripping quotes and a namespace. Reuse it; no new type parser. | `src/program/resolve/receiver.rs:444-560,3426-3436` |
| P9 real BC test code | The CDO repo has a test app at `/work/src/cdo-baseline/Test` (same repo as the pinned `CDO_WS`, HEAD `bc3ccb1`; depends on "Library Assert", "Tests-TestLibraries"). 139 files, 1847 `[Test]` lines. Assert variables: 56 `Codeunit Assert`, 53 `Codeunit "Library Assert"`; names `Assert` (105) and `LibraryAssert` (4). **All 109 are object globals** (8-space indent under the object `var`), none local. Numeric form `Codeunit 130000/130002`: 0 uses. Methods used: AreEqual 1476, IsTrue 1190, IsFalse 431, AreNotEqual 112, RecordCount 69, ExpectedError 62, RecordIsEmpty 12, ExpectedErrorCode 5, RecordIsNotEmpty 4, Fail 2, ExpectedTestFieldError 1. `asserterror`: 104. | grep over `/work/src/cdo-baseline/Test/**/*.al` |
| P10 helper-only tests | A rough line-based count (mawk, not the IR): 1844 tests, **88 (4.8%) have no direct `Assert.`/`asserterror`** in their own body. Most likely assert through `Verify...` helpers or library codeunits (`src/TestFramework/CDOLibrary*.Codeunit.al` hold `LibraryAssert` calls). These will show `zero_assert: true`. Exact number comes from the measurement plan. | awk run over the Test app |
| P11 "literal expected" shapes | Probe: `Enum::"Sales Document Type"::Order` and `S."Document Type"::Order` both lower to `ExprKind::QualifiedEnum` (`qualified_enum_value`). `-5` lowers to `Unary { Neg, Literal(Int) }`. Literals: `Literal::{Int, Decimal, Bool, Text, Date, DateTime, Time, Other}`. Rough CDO split of AreEqual/AreNotEqual first arguments: 970 plain literal, 67 `::` enum/option, 551 other. | probe `aldump --l2`; `crates/al-syntax/src/ir/expr.rs:76-87,128-132`; grep |
| P12 existing analyze tests and goldens | `tests/cli/cli_analyze_call_hierarchy_json.rs` (#159) drives the shipped binary (`CARGO_BIN_EXE_al-call-hierarchy`) on `tests/fixtures/analyze-json/`. `legacy_fields_text_and_csv_byte_identical_to_base` compares the WHOLE parsed JSON against FROZEN `tests/fixtures/analyze-json-expected/legacy.json` after removing only the #159 row keys and `unused_procedure` findings. **A new top-level key breaks that test** unless it is also removed there. The frozen files must not be regenerated. The fixture already holds one empty `[Test] TestSomething` (`src/C/Tests.Codeunit.al`). Not one of the 23 golden directories; `check-goldens` is unaffected. | test file lines 1-14, 520-586 |
| P13 versioning | The analyze payload has no `schemaVersion`. #159 versioned by "append, never reorder" (new row keys after `quality_score`, new findings after old ones) and a CHANGELOG entry. | `src/analysis.rs:5-6,317-318` |
| P14 `routineId` / `objectId` vs #159 | #159 rows carry `routine_id` (alsem `stableRoutineId`, `null` without app guid / nested app / nameless) and `object_id` (the object NUMBER, `null` when none), plus `path`, `line`, `object_type`, `object_name`, `procedure_name`. All keys are snake_case. The issue's `"<appId>/Codeunit/80100"` is the internal object id form (`ids::encode_object_id`); the stable form is `guid:Codeunit:80100`. Neither is in today's output. Decision: reuse the #159 keys and values exactly (`routine_id`, `object_id`, `path`, `line`), so a test row joins its metrics row on `(path, line)` or `routine_id`. | `src/analysis.rs:8-38,386-450`; `src/engine/ids.rs:177-193`; `src/program/body/l2_workspace.rs:61-75` |
| P15 determinism | `al-call-hierarchy` has no `--deterministic` flag (that is `alsem`). The analyze output is already stable: `discover_al_files` returns files sorted by `rel_posix`, `par_iter().collect()` keeps that order, objects and routines are in source order, no hash-map iteration reaches the output. | `src/source_text.rs:61`; `src/main.rs` Args |
| P16 cost baseline | `al-call-hierarchy --project /work/src/cdo-baseline/Test --analyze --format csv` (target/cdo build of 10-22): exit 0, 139 files, 2586 procedures, "Analyzed ... in 44.2ms". **Not a valid baseline** (O6/S4): that binary (2026-10-22 06:15) predates #159 (`809fe684`, 2026-10-23 13:55); its CSV has no #159 fields; CSV builds no graph. Replaced by the JSON master-vs-branch measurement in the Design. | scratchpad `cdo-test.err`, `cdo-test.csv` header; `git log -1 809fe684` |
| P17 legacy test raw scan (S1/O1) | Confirmed. The test opens a "row" at any line starting `"object_type"` and closes it at the next line starting `}`, over the WHOLE stdout. New `test_quality` rows carry `object_type`, and the fixture's `[Test] TestSomething` makes `procedures` non-empty, so the key-order and `row_count` asserts would fail. | `tests/cli/cli_analyze_call_hierarchy_json.rs:543-569`; `tests/fixtures/analyze-json/src/C/Tests.Codeunit.al:5` |
| P18 block items and statement slots (O2/S2) | Confirmed. A block holds `BlockItem::{Stmt, Preproc(PreprocGroup{branches})}`; both `#if` arms are lowered and kept, directives not evaluated. `StmtKind` has 16 variants with these expression/block slots: Assignment{target,value}, Call(e), If{cond,then,else?}, Case{scrutinee, branches[patterns, body], else?}, While{cond,body}, Repeat{body,until}, For{var,from,to,body}, Foreach{var,iterable,body}, With{receiver,body}, Try{body,catch?}, AssertError(block), Exit(e?), Block(b), Break, Continue, Unknown. `complexity_stmt` ends in `_ => {}`. | `crates/al-syntax/src/ir/stmt.rs:15-94`; `src/analysis.rs:509-607` |
| P19 resolved callees for the reachable count | Cheap. With `--format json`, `analyze_project` builds `LspSnapshot` and `Graph`. `snap.edges_by_file` holds every workspace caller's `Call`/`Run`/`ImplicitTrigger` edges (`LspEdge{from: RoutineNodeId, targets: Vec<LspTarget>}`); `LspTarget::Routine(id)` or `Abi(..)`. `snap.decls_by_file` holds only workspace declarations (`DeclEntry{id, origin}`), and `file_metrics` already joins a routine to its `DeclEntry` by `(rel_posix, origin.byte.start)`. Nested-app files are part of the root app's source root (`walk_al_source` uses `NestedApps::Walk`), so "same app" = "has a `DeclEntry`". `Graph.fan_out` also mixes in `ws_event_edges`; the reachable count must not reuse it (calls only). | `src/lsp/snapshot.rs:113-116,313-322,459-466,521`; `src/lsp/handlers.rs:176-185`; `src/snapshot/provider.rs:31-47`; `src/analysis.rs:355-365,405-407` |
| P20 `this.` receiver (O3) | `this` lowers to `ExprKind::Identifier("this")` (the resolver matches it that way), so `this.Assert.AreEqual(..)` is `Call{Member{Member{Identifier("this"),"Assert"},"AreEqual"}}`. CDO: 0 uses. | `src/program/resolve/receiver.rs:2357-2364` |
| P21 commented-out tests (O5) | Confirmed: 3 of the 1847 grep hits are `// [Test]` (`CDOOpenDocumentsTests.Codeunit.al:312`, `CDOSendCustStatementTests.Codeunit.al:244,401`). Expected count is exactly 1844. | grep |
| P22 numeric ids 130000/130002 (O9) | Not cheaply verifiable. The BC sidecar is stopped and reported "exited unhealthy" (`kraken siphon status bc`); a cold start takes ~3 min with no health guarantee. No test-library `.app` exists under `/work` or `/home/dev` (813 `.app` files, none named test/assert). CDO uses the numeric form 0 times. Decision: cut the numeric form. | `kraken siphon status bc`; `find` |
| P23 in-body `#if` in the IR (S2/O2 r2) | Confirmed. `lower_block_child` flattens every LIVE branch's children into the enclosing block's items; no lowerer path constructs `BlockItem::Preproc` (grep finds only `match` arms in consumers). Liveness: a branch is dead only when the build context (empty at top level, extended per enclosing branch) decides it false. `Ir::add_block`/`add_stmt`/`add_expr` are `pub`, so a unit test can build a `Preproc` block by hand. | `crates/al-syntax/src/lower/mod.rs:1389-1445,2367-2387`; `crates/al-syntax/src/ir/mod.rs:100-110` |
| P24 no-app fixture (S3 r2) | Confirmed: `tests/fixtures/analyze-json-noapp/` holds only `Solo.Codeunit.al`, with no `[Test]`. | `ls`, grep |
| P25 one id per signature (S8/O10) | Confirmed: `source_routine_node_id` builds the id from object, name, enclosing member, param count and param-type fingerprint; no location or preprocessor context. `updater.rs` keeps `decl_multiplicity` for exactly this case. | `src/program/sig_fp.rs:155-166`; `src/lsp/updater.rs:310-315` |
| P26 nested apps in the graph (S7) | Confirmed: the workspace source root walks nested apps (`NestedApps::Walk`), so their routines have `DeclEntry`s; #159 treats a nested-app file as another app for ids (`analysis.rs:291-295`). | `src/snapshot/provider.rs:31-47`; `src/analysis.rs:291-295,327-337` |

## Design

Version 3 (after spec panel round 2). Register: `findings.json` in this folder.

### Goal

Add a `test_quality` block to `al-call-hierarchy --project <ws> --analyze --format json`.
It lists every `[Test]` procedure with its assert counts, and sums them per test
codeunit and for the workspace. CentralGauge diffs it between the start tree and the
final tree.

### Non-goals

- No mutation testing, no compile, no run.
- No new flag, no new alsem command, no text/CSV change, no `schemaVersion`.
- No counting of `Error(...)` used as a hand-made assert, and no other test libraries
  (`Library - Utility` etc.).
- No following of event links (publisher to subscriber) in the reachable count. Only
  call edges. An event subscriber is not "called by the test".
- No numeric assert receiver (`Codeunit 130000` / `Codeunit 130002`): cut, because the
  ids cannot be checked against symbols here and CDO never uses that form (P22).

### Output shape

Appended after `summary`, so existing keys keep their order. All keys snake_case,
like the rest of this payload (operator answer 1).

```json
"test_quality": {
  "procedures": [
    { "path": "src/T.Codeunit.al", "line": 8, "object_type": "Codeunit",
      "object_name": "Probe Tests", "object_id": 80100,
      "object_stable_id": "<appGuid>/Codeunit/80100",
      "procedure_name": "T1", "routine_id": "<same value as the metrics row>",
      "assert_count": 3, "assert_count_reachable": 5, "zero_assert": false,
      "has_asserterror": true,
      "are_equal": { "total": 2, "literal_expected": 1, "expression_expected": 1 } }
  ],
  "codeunits": [
    { "path": "src/T.Codeunit.al", "line": 1, "object_type": "Codeunit",
      "object_name": "Probe Tests", "object_id": 80100,
      "object_stable_id": "<appGuid>/Codeunit/80100",
      "test_procedures": 6, "assert_count": 14, "assert_count_reachable": 17,
      "zero_assert_procedures": 1, "asserterror_procedures": 2,
      "are_equal": { "total": 9, "literal_expected": 5, "expression_expected": 4 } }
  ],
  "workspace": { "test_procedures": 12, "assert_count": 30,
    "assert_count_reachable": 36, "zero_assert_procedures": 2,
    "asserterror_procedures": 4,
    "are_equal": { "total": 20, "literal_expected": 11, "expression_expected": 9 } }
}
```

Field rules:

- `path`, `line`, `object_type`, `object_name`, `object_id`, `procedure_name`,
  `routine_id` on a procedure row are the exact values of that routine's `metrics`
  row. The same loop computes both, so they join on `(path, line)`. `routine_id` is
  `null` in the same cases as in `metrics`.
- `object_stable_id` (operator answer 2): `ids::encode_object_id(guid,
  ir_object_type(kind), ir_object_key(obj))`, the internal `<appGuid>/Codeunit/N` form
  (`@name` key for an object without a number). `null` exactly when the object part of
  `routine_id` is `null`: no root app guid, a file in a nested app, or a kind with no
  object type. It reuses the guid `file_metrics` already filters for nested apps.
- `codeunits[]`: one entry per object that holds at least one `[Test]` procedure;
  `line` is the object's start line.
- `workspace` is always present; all zeros when there are no tests.
- `assert_count` = the body's own count. `assert_count_reachable` = own count plus
  reachable helpers (rule 7). `zero_assert` = `assert_count_reachable == 0`.
- Without a call graph (no `app.json`, or the snapshot cannot be built, or the routine
  has no matching declaration), `assert_count_reachable` and `zero_assert` are `null`,
  never a guess. This matches #159's "null, not 0" rule for `fan_in`/`fan_out`.
- Sums: `assert_count`, `test_procedures`, `asserterror_procedures` and `are_equal`
  always sum. `assert_count_reachable` on a codeunit or the workspace is `null` if any
  of its procedures has `null`, else the sum. `zero_assert_procedures` follows the
  same null rule (O11): `null` if any of its procedures has `zero_assert: null`, else
  the number of procedures with `zero_assert == true`. No made-up 0.
- Order: `procedures` and `codeunits` follow the `metrics` order: path (sorted), then
  source order. This replaces the issue's "object id, then line": an object id can be
  null or repeat across nested apps, and path order is already deterministic and joins
  `metrics` 1:1. Codeunit sums are built in that order, never by iterating a hash map.
  Output is byte-identical across runs without any flag.

### Counting rules

1. **Test procedure**: a routine with `"test"` in `r.attributes` (lowercased by the
   lowerer), any object kind. Other attributes (`HandlerFunctions`, `TestPermissions`,
   handlers) do not matter: a routine with only `[TestPermissions]` or a
   `[ConfirmHandler]` is not a test. The file set is the `metrics` file set, nested
   apps included. A test written in both arms of an object-level `#if` is two
   declarations and gives two rows.
2. **Assert receiver**: a call `X.M(...)` where the receiver is either
   - a bare `Identifier` or `QuotedIdentifier` `X`, looked up case-insensitively in
     AL scope order: the routine's locals, then its parameters, then the object's
     globals; or
   - `this.X` (`Member { Identifier("this"), X }`), looked up in the object's globals
     only (O3; 0 CDO uses, but agent code uses `this.`).

   and `X`'s declared type classifies (`classify_type_text`) as `Codeunit` named
   `assert` or `library assert` (quotes and namespace stripped). The rule is by type,
   not by the name `Assert`. Any method counts (`AreEqual`, `IsTrue`, `ExpectedError`,
   `RecordCount`, ...). A local `Assert` of another type hides a global assert, so its
   calls do not count.
3. **`asserterror`**: each `StmtKind::AssertError` adds 1, and the walk goes on into
   its block. Nested ones each count. An assert call inside the block counts too.
   `asserterror X; Assert.ExpectedError(..)` counts 2, as the issue defines.
4. **Own count** (`assert_count`): assert calls + `asserterror` statements in the
   routine's own body. `has_asserterror` = at least one `asserterror` in the own body.
5. **`are_equal`** (own body only): assert calls whose method is `AreEqual`,
   `AreNotEqual` or `AreNearlyEqual` (case-insensitive). `literal_expected` when the
   FIRST argument, after removing `Parenthesized`, is:
   - a `Literal` of any kind (number, text incl. `''`, boolean, date/time),
   - a `Unary` `Neg`/`Plus` over a numeric literal (`-5`),
   - a `QualifiedEnum` (`Enum::"X"::Y`, `Rec.Field::Y`, an option value), or
   - a `DatabaseReference` (`Database::"Customer"`), a compile-time constant (O4).

   Everything else is `expression_expected`: variables, fields, calls, arithmetic,
   `Label`/`Text` constants (a named constant is a variable in syntax), and a call with
   no first argument. `total = literal_expected + expression_expected` always.
6. **The walker** is a new private visitor. It does NOT copy `complexity_stmt` (which
   ends in `_ => {}`). It has:
   - an exhaustive `match` on `BlockItem`: `Stmt` is visited; `Preproc` visits EVERY
     branch. Policy: an in-body `#if` / `#else` with the same assert in both arms counts
     it twice (both arms are source; the walker does not evaluate directives).
     **Which path real AL takes (P23):** the lowerer never emits `BlockItem::Preproc`
     today. `lower_block_child` (`lower/mod.rs:2367-2387`) flattens every LIVE `#if`
     branch's statements into the enclosing block as plain `BlockItem::Stmt`s. Under
     the empty top-level build context every branch is live, so both arms' asserts
     reach the walker as ordinary statements and count twice. A branch is dead only
     when an enclosing branch already decided the same symbol the other way (e.g.
     `#if X` inside `#else` of `#if X`); its statements are not in the IR and do not
     count. The `Preproc` arm is kept for IR built another way and is pinned by a
     synthetic-IR unit test (U1), not by a fixture;
   - an exhaustive `match` on `StmtKind` with no catch-all arm, visiting every slot:
     `Assignment` target and value; `Call` expr; `If` cond, then and else; `Case`
     scrutinee, every branch's patterns and body, and else; `While` cond and body;
     `Repeat` body and until; `For` var, from, to and body; `Foreach` var, iterable
     and body; `With` receiver and body; `Try` body and catch; `AssertError` block
     (+1); `Exit` value when present; `Block`; `Break`, `Continue`, `Unknown` have no
     slots (explicit empty arms);
   - expressions recurse through `ExprKind::children()` (complete; `Unknown` is a
     leaf), and every expression slot above is visited. In valid AL an assert call
     cannot sit inside a condition or another expression, because the assert
     codeunits' methods return nothing. So this recursion is defensive and has no
     discriminating fixture of its own. What A10 pins is that a non-assert call nested
     in an assert's argument (`Assert.IsTrue(Rec.IsEmpty(), '')`) is not counted, and
     the assert itself counts once.

   A bare call inside `with Assert do` is not resolved (deprecated construct; CDO has
   0 uses). Stated as a limit.
7. **Reachable count** (`assert_count_reachable`, operator answer 3). Chosen: it fits
   the budget (P19). Rule:
   - Compute the own count (rule 4) for EVERY routine with a body, not only tests,
     using that routine's own scope (its locals, params, its object's globals). Key it
     by `(path, byte start)`, the join `file_metrics` already uses for `DeclEntry`.
   - Build the call map keyed by `(app root, RoutineNodeId) -> targets` from
     `snap.edges_by_file` only (`Call`, `Run`, `ImplicitTrigger`; all targets of an
     edge, so interface dispatch follows every candidate). Not from `Graph.fan_out`,
     which mixes in event links. The app root is the CALLER file's root
     (`edges_by_file` is per file), so a root-app and a nested-app declaration that
     share one `RoutineNodeId` (the id holds no file or root, `sig_fp.rs:155-165`)
     keep their outgoing calls apart; the walk reads only the test's own root's
     entries (S7, round 3). Fixture: A14b, a root routine and a nested routine with
     the same signature, the nested one calling a root helper that holds an assert;
     a root test calling the root routine gets no assert from the nested one's call.
     Break: key the map by `RoutineNodeId` alone -> the root test counts it; fails.
   - **Same app (S7).** Each file has an app root: the deepest folder at or below the
     project that holds an `app.json` and contains the file, else the project root.
     This is #159's nested-app attribution (`in_nested_app`, `analysis.rs:291-295,
     327-337`) generalised from "is nested" to "which root"; one small helper next to
     `in_nested_app`, both reading the same `app.json` probes. A target is followed
     only when it is `LspTarget::Routine(id)`, has at least one `DeclEntry` in
     `snap.decls_by_file`, and that declaration's file has the SAME app root as the
     test's file. So a root-app test does not count a nested app's helpers, and a
     nested-app test counts only helpers of its own nested app. Dependency and `Abi`
     targets (including the `Library Assert` codeunit itself) are never followed. The
     CHANGELOG says "same app" with this meaning.
   - **One id, several declarations (S8/O10).** `RoutineNodeId` has no source location
     (`sig_fp.rs:155-166`), so the same signature in two object-level `#if` arms is
     one id with two `DeclEntry`s (`decl_multiplicity` exists for this). Edges are
     keyed by that id, so the id's callees are the UNION of both arms' calls. Rules:
     - a callee id's own count is the SUM over all its declarations (same app root),
       consistent with "both arms count" for in-body `#if`;
     - a test contributes only ITS OWN declaration's count (by `(path, byte start)`),
       and its id is in the visited set from the start, so the other arm's
       declaration is never added for this test;
     - a split test therefore reaches the union of both arms' callees. Stated as a
       limit in the CHANGELOG (the two arms are never both compiled, but the graph
       does not tell them apart).
   - For each test: a worklist walk from the test's `RoutineNodeId` with a visited
     set (seeded with the test's id). `assert_count_reachable` = test's own
     declaration count + the summed own count of every OTHER visited id. Each id
     counts once per test, so a helper called twice (or from two other helpers)
     counts once. Cycles end because of the visited set.
   - Sums do not depend on visit order. Cost: one walk per test over workspace
     routines (CDO: 1844 tests x at most 2586 routines; measured, see below).
   - If the measurement or the S7/S8 rules turn out larger than the half-day budget,
     the fallback below still applies (ship body-only, file a discovery issue).

### Where it lives

- New file `src/test_quality.rs` (`pub mod test_quality;` in `lib.rs`): the row and
  summary structs, the walker, the literal classifier, the reachable walk and the
  sums.
- `src/analysis.rs`: `file_metrics` also returns, for every routine with a body, its
  own count keyed by `(path, byte start)`, and a test row for each `[Test]` routine,
  built in the same `for obj / for r` loop. `analyze_project` joins them in file order,
  runs the reachable walk when the graph exists, builds `codeunits` and `workspace`, and
  sets the new last field `AnalysisResult.test_quality`. Text and CSV printers ignore
  it.
- `tests/cli/cli_analyze_call_hierarchy_json.rs`, test
  `legacy_fields_text_and_csv_byte_identical_to_base` (S1/O1), two edits:
  1. remove the top-level `test_quality` key from the parsed value before the compare;
  2. stop the raw-line key-order scan at the end of the `metrics` array (break at the
     line `"findings": [`).
  The frozen files `tests/fixtures/analyze-json-expected/legacy.*` stay unchanged.
- `src/analysis.rs` also gets the app-root helper (S7) next to `in_nested_app`.
- `CHANGELOG.md` `Added` entry: the block, the counting rules in one paragraph,
  "same app" as defined in rule 7 (the test's own app root; nested apps and
  dependencies are not followed), and the limits (event links not followed, `with`
  not resolved, numeric receivers not matched, `Label` expected values count as
  expressions, a test split across object-level `#if` arms reaches the union of both
  arms' callees, unresolved calls not followed).
- `README.md:50-51` (S6/O8): add "test assert counts (JSON)" to the list of what
  `--analyze` reports.

### Acceptance matrix (all through the shipped binary)

New fixture `tests/fixtures/analyze-test-quality/` (`app.json` with an app id, plus
test codeunits, a helper codeunit and one nested app folder `nested/` with its own
`app.json`). Second new fixture `tests/fixtures/analyze-test-quality-noapp/` (no
`app.json`; one test codeunit with two `[Test]` procedures, one with an assert, one
without). The existing `analyze-json-noapp` has no `[Test]`, so it cannot serve (S3,
P24); it is not edited. New member `tests/cli/cli_analyze_test_quality.rs` in the `cli`
umbrella, run as `CARGO_BIN_EXE_al-call-hierarchy --project <fixture> --analyze
--format json`. Every expected number is hand-counted from the fixture text and
written in the test.

Every break below is made with Edit, the test is run and must go red, then the break
is reverted and the test must go green. Both outcomes are recorded in this ledger.

| # | Issue item / finding | Fixture shape | Test asserts | Break → expected red |
|---|-----------|---------------|--------------|-------------------------------------------|
| A1 | 1 + S3 | 7+ tests over 2 codeunits, helpers, handlers, one test in a nested-app folder | the set of `(path, procedure_name, line)` equals the hand list; for EVERY procedure row, exactly one `metrics` row has the same `(path, line)`, and `object_type`, `object_name`, `object_id`, `procedure_name`, `routine_id` are equal to it, including `routine_id: null` for the nested-app test; `object_stable_id` equals `<fixture guid>/Codeunit/<n>` and is `null` for the nested-app test | (a) drop the `"test"` check → set differs; (b) set the test row's `routine_id` to `None` → join red; (c) use the object's start line as the test row's `line` → join red; (d) build `object_stable_id` from the unfiltered root guid → nested-app `null` assert red |
| A1b | S3 + O11 null case | `analyze-test-quality-noapp` | `procedures` has exactly 2 rows (non-empty precondition); each has `routine_id`, `object_stable_id`, `assert_count_reachable`, `zero_assert` all `null` and numeric `assert_count` (1 and 0); the codeunit row and `workspace` have `assert_count_reachable: null` and `zero_assert_procedures: null`, and numeric `assert_count` 1 | (a) emit `zero_assert` from `assert_count` when the graph is missing → red; (b) count only `true` for `zero_assert_procedures` (the old rule) → workspace gives 0 → red |
| A2 | 2 + O7 | zero-assert test whose body calls `Rec.Init(); Rec.Insert();` and nothing else | `assert_count 0`, `assert_count_reachable 0`, `zero_assert true` | count any `X.M()` call as an assert → red |
| A2b | 2 | `asserterror Error('x');` only | `assert_count 1`, `has_asserterror true`, `zero_assert false` | remove the `AssertError` +1 → red |
| A3 | 3 | `AreEqual(5,..)`, `AreEqual('',..)`, `AreEqual(-5,..)`, `AreEqual(Enum::"X"::Y,..)`, `AreEqual(Database::Customer,..)`, `AreEqual(Expected,..)`, `AreEqual(Rec.Amount,..)`, `AreNotEqual(Format(X),..)`, `AreNearlyEqual(1.5, D, 0.01, '')` | literal 6, expression 3, total 9 | drop the `QualifiedEnum` arm → literal 5; drop the `Unary` arm → literal 5; drop `arenearlyequal` from the method list → total 8 |
| A4 | 4 | (all of the above) | each codeunit's numbers equal the sum of its procedures, and workspace equals the sum of all, computed in the test from the JSON; also checked against hand totals | (S9) leave the LAST procedure of each codeunit out of the codeunit sums; the fixture's last test in every codeunit has `assert_count > 0` and `assert_count_reachable > 0` (asserted as a precondition) → both the computed-sum and the hand-total checks red |
| A5 | 5 | | two runs give byte-identical stdout; `procedures` order equals `(path, line)` order | build `codeunits` by iterating a `HashMap` → order assert red (if hash order happens to match, record that honestly; the `(path, line)` order assert is the guard) |
| A6 | 6 | `[TestPermissions(..)]`-only, `[ConfirmHandler]`, `[MessageHandler]`, `local procedure VerifyX()` | none of them appear as rows | treat `is_framework_invocation_attribute` as "test" → handlers appear |
| A7 | 7 + S8/O10 | object-level `#if CLEAN25 [Test] procedure T() begin Assert.IsTrue(true, ''); HelperA(); end; #else [Test] procedure T() begin Assert.IsTrue(true, ''); Assert.IsFalse(false, ''); HelperB(); end; #endif`; `HelperA` has 1 assert, `HelperB` 2. Plus a helper `S()` split the same way (arm 1: 1 assert, arm 2: 2 asserts) called once by test `U` (0 own asserts) | both T rows appear. Arm 1: `assert_count 1`, `assert_count_reachable 4` (own 1 + HelperA 1 + HelperB 2: the union). Arm 2: `assert_count 2`, `assert_count_reachable 5` (own 2 + 1 + 2). `U`: `assert_count_reachable 3` (S's two declarations summed). Precondition: the two `T` rows share one `routine_id` (proves the shared id) | (a) take a test's own count as the id's SUM → arm 1 gives 6 → red; (b) keep one declaration per id (map insert, last wins) → `U` gives 2 → red; (c) do not seed the visited set with the test's id → no change expected for T (no self-call); recorded as not discriminating, (a) guards the rule |
| A8 | receiver by type | global `Assert: Codeunit Assert`; global `LibraryAssert: Codeunit "Library Assert"`; a test with a local `Assert: Codeunit "Other"` hiding the global; a local `Asserter: Codeunit "Library Assert"`; a test using `this.Assert.IsTrue(..)` | hidden calls not counted; renamed and `this.` calls counted | look up globals before locals → hiding test red; match by name `Assert` instead of type → `Asserter` red; remove the `this.` arm → red |
| A9 | S2 parameter receiver | helper `local procedure Check(A: Codeunit "Library Assert")` calling `A.IsTrue(..)`, called by a test | the helper's assert reaches the test's `assert_count_reachable` | drop params from the lookup → red |
| A10 | S2 / O2 traversal | one test with one assert in each of: `if` then, `if` else, a `case` branch body, `case` else, `while` body, `repeat` body, `for` body, `foreach` body, a nested `begin ... end` block, and `asserterror begin Assert.Fail(''); end`; plus `Assert.IsTrue(Rec.IsEmpty(), '')` (an assert whose argument holds a non-assert call) | `assert_count` equals the hand count (12: 10 slot asserts + 1 `asserterror` + 1 `IsTrue`; the inner `Rec.IsEmpty()` is not counted) | make each of these arms empty in turn: `If` else slot, `Case` branch body, `Case` else, `While` body, `Repeat` body, `For` body, `Foreach` body, `Block`, `AssertError` descent → each drops the count → red. Count an assert's nested calls as asserts → 13 → red |
| A11 | O2 in-body `#if` (real AL path) | a test body with `#if CLEAN25 Assert.IsTrue(..) #else Assert.IsTrue(..) #endif` | counts 2 (policy). This pins the FLATTENED path real AL takes (P23): both arms arrive as plain statements | not a test of the `Preproc` arm (that is U1). Break: none in the walker can make it red except dropping statements; recorded as "documents behaviour" |
| U1 | S2/O2 `Preproc` arm | unit test in `src/test_quality.rs`: parse a routine with two assert statements, then build a new block by hand (`Ir::add_block`, public) whose single item is `BlockItem::Preproc(PreprocGroup { branches: [b1, b2] })`, each branch a block holding one of the two assert statements | the walker over that block counts 2 | (a) make the `Preproc` arm empty → 0 → red; (b) visit only the first branch → 1 → red |
| A12 | operator 3 reachable | test T calls helper H twice; H calls H2; H2 calls H (cycle); H has 1 assert, H2 has 2; T has 0 own; a second test calls a helper in another codeunit of the same app; a third test calls only `Assert.IsTrue` directly plus a platform method (targets with no `DeclEntry`) | T: `assert_count 0`, `assert_count_reachable 3`, `zero_assert false`; cross-codeunit helper counted; third test `assert_count 1`, `assert_count_reachable 1` (nothing outside the app is followed) | (a) (S10/O12) dedup break that cannot loop: replace the per-test visited set with a per-PATH guard (skip only routines already on the current call path) and walk every call EDGE (each call site, not the deduplicated target set) → T's two call sites to H each give H 1 + H2 2 (H2→H is cut by the path guard) → T gives 6 → red, and the walk ends because every path is finite; (b) follow only direct callees (no transitive step) → T gives 1 → red; (c) follow event edges too: a fixture subscriber with an assert on a publisher the test raises must NOT be counted → break by building the map from `Graph.fan_out` → red |
| A14 | S7 same app | in `analyze-test-quality/nested/` (own `app.json`): codeunit with `procedure NestedHelper()` holding 1 assert; a ROOT test `V` (0 own asserts) calls it | `V`: `assert_count_reachable 0`, `zero_assert true`. Precondition: `NestedHelper`'s `metrics` row has `fan_in >= 1` (the call resolves, so the filter is what excludes it) | drop the app-root comparison → `V` gives 1 → red |
| A13 | S1/O1 #159 contract | existing `analyze-json` fixture | the legacy test, with its two edits, is green; text/CSV byte-identical | revert edit 2 only → legacy test red (proves the scan edit is needed) |

Unit tests in `src/test_quality.rs` cover the literal classifier on IR from
`al_syntax::parse`, one per shape, each with a break/revert record.

Every fixture file must parse without error (S11). The test member walks every `.al`
file under both new fixtures, parses it with `al_syntax::parse` (integration tests
already use `al_syntax`, e.g. `tests/cli/cli_numberless_object_ids.rs`), and asserts
that it has at least one object and that every routine has `parse_incomplete ==
false`. Matching `metrics` rows alone is not enough: recovery can keep a declaration
while dropping body calls (`lower/mod.rs:2107`). The `Try` statement kind has no
plain-AL fixture shape here; its arm is covered by the exhaustive match only
(stated).

The binary runner needs no timeout for these breaks: every break above ends (S10's
looping variant is replaced by the path-guarded one).

### False-positive and failure shapes

- **Helper asserts**: covered by `assert_count_reachable` when the graph exists. A
  helper in a dependency app or in another app root of the workspace (a nested app)
  is not followed (stated).
- **Test split across object-level `#if` arms**: reaches the union of both arms'
  callees (rule 7). Stated.
- **Unresolved call** to a helper (an `unknown` edge): not followed; the count may be
  low. Stated as a limit; CDO measurement reports how many `zero_assert` tests remain.
- **`asserterror` without a following `ExpectedError`**: counts 1, as defined.
- **Nested `asserterror`**: each counts.
- **Assert variable declared globally** (all 109 CDO cases), locally, as a parameter,
  or used through `this.`: all found (rule 2).
- **`[HandlerFunctions('X')]` on a test**: still a test. The handler itself is not a
  row. A handler reached only through the test framework has no call edge, so its
  asserts are not in the reachable count. Stated as a limit.
- **Interface dispatch**: every candidate implementation is followed, so a reachable
  count can be high. Stated.
- **Malformed routine** (`parse_incomplete`): counted from whatever IR exists.

### Measurement plan

On `/work/src/cdo-baseline/Test` (the CDO repo's test app, `bc3ccb18`, 139 files):

- Build master (`60434aef`) and the branch with the same profile
  (`--profile release-fast`), same machine, no `.alpackages` in either run (the app
  ships none), warm file cache (one discarded run each). Run each 5 times with
  `--analyze --format json` and report median wall time of both (S4/O6).
- Report: test procedures (must be exactly 1844; anything else is a bug, P21),
  `zero_assert_procedures` with own count and with reachable count (rough own-count
  expectation 88, P10), `asserterror_procedures`, `assert_count`,
  `assert_count_reachable`, and the `are_equal` split (rough grep expectation: ~1037
  literal incl. enum, ~551 expression, of ~1588 AreEqual/AreNotEqual calls).
- Hand-check 5 tests that stay `zero_assert: true` against the source. Each must be a
  real zero (no direct assert, no resolved same-app helper with one), not a walker or
  graph miss.
- Explain every difference from the grep numbers; do not just accept it.

### Fallbacks, stated in advance

- If the reachable walk is too slow on CDO (branch median more than 2x master's JSON
  time), memoize per-routine reachable sets bottom-up only if cycles allow; otherwise
  ship body-only, state the limit in the CHANGELOG, and file one discovery issue for
  the reachable count. Decide on the measured number, not before.
- If the CDO hand-check finds a `zero_assert` test that does assert directly (a walker
  miss), fix the walker before shipping.
- If a real assert receiver shape is missed and it shows up in the CDO measurement,
  fix it; otherwise record it as a limit in the CHANGELOG.
- If CentralGauge later wants camelCase: a serde rename on the new structs only. The
  object key is already provided by `object_stable_id` (S5), so no logic change is
  tied to the casing.

### Open questions

None blocking. Operator answers 1-3 settle casing, the object key and helper asserts.

## Timeline (conductor)

- Operator answers via alsem-main (2026-10-23): snake_case; add `object_stable_id`; add `assert_count_reachable` (same-app resolved calls) if it fits ~half a day, `zero_assert` = zero reachable asserts; else body-only + discovery.
- Spec round 1: sol S1-S6, opus O1-O9; both "do not approve". v2 (design author, opus): reachable count built (P19), 15/15 fixed.
- Spec round 2: opus approved (O10-O12 minor); sol re-raised S2/S3/O2, new S7-S11. Conductor decisions: real same-app filter by app root (S7); a callee id's own count = sum over its declarations, a test counts its own declaration (S8/O10). v3.
- Spec round 3: opus approved all 11; sol accepted 10, re-raised S7 (root and nested declarations can share a RoutineNodeId, merging outgoing calls). Conductor edit: call map keyed by (caller app root, RoutineNodeId); fixture A14b.
- Spec round 4: both accepted S7; both approve. Opus's wording note for the implementer: A14b's two declarations collide only with the same object kind AND number (ObjectNodeId{app, kind, key}), not just the same procedure signature. Register: 23 entries, all accepted by both. spec_rounds remaining 1.

## Implementation

Plan: `docs/superpowers/plans/2026-10-23-issue-253-test-quality.md`, **2 tasks**: body-only
metrics, then the reachable count.

### TDD red (before any implementation)

`cargo nextest run --test cli cli_analyze_test_quality::` on base code + new tests/fixtures:
11 of 12 FAIL, every one at `cli_analyze_test_quality.rs:29` "no `test_quality` key in the
analyze JSON". `fixtures_parse_without_error` PASSES, as it should: it checks the fixture,
not the feature (10 `.al` files, no `parse_incomplete`).

### Fixture notes

- A14b: root `TQ Twin` and nested `TQ Twin Nested` are both codeunit 50260 (the round-4
  note: same kind AND number). A name-typed variable (`Codeunit "TQ Twin"`) did not
  resolve with the duplicate number (fan_out 0); the test's variable is `Codeunit 50260`,
  which resolves (fan_in of root `Twin` is 1, asserted as a precondition).
- **Deviation (conductor decision, option A): the nested `app.json` is committed as
  `nested/app.json.tmpl`.** The test member copies the fixture into a tempdir and writes
  `nested/app.json` there, as #159's nested-app test does. Why: with a committed nested
  `app.json`, the #160 T2 sweep `program_resolve_harness::adapter_loses_nothing_cross_app_on_the_fixtures`
  failed on the fixture (`program_only_site 4, adapter_l3_fallback_sites 1,
  adapter_callee_outside_l3 1`). Bisect with fixture copies and a temporary census probe
  (since removed): full fixture = those losses; without only the duplicate-50260 twin =
  still losses (3/1/1); without the `nested/` folder = 0 losses. So the nested app alone
  causes it. That divergence is deliberate and pinned by
  `program_calls::tests::nested_app_json_folder_is_program_only`. No committed fixture had a
  nested app before. `program_resolve_harness` is not touched.
- `FIXTURE_ROOTS` (`src/engine/l4/capability_cone.rs`) `("fixtures", 36)` -> 37: the
  stated tripwire for a new fixture app dir. Both oracles that read it
  (`relation_equals_the_legacy_base_per_field`, `derived_only_matches_raw_fold_on_fixtures`)
  pass with the new dir.

### Discrimination proofs

Every break was made with Edit and reverted with Edit. After each group of reverts the file
was `cmp`-identical to a saved clean copy, and the suites were re-run green (29/29 analyze
CLI tests, 26/26 lib `test_quality::`+`analysis::`). Test runs ran the shipped binary. The
"pass" column refers to that verification. A1(d), A14 and A14b were run AGAIN after the
tempdir change (option A), and each went red there too (same assertions).

| test | break | fail output | pass |
|------|-------|-------------|------|
| A1 | (a) `is_test` also true for any routine with a body | set differs (handlers, helpers appear) | yes |
| A1 | (b) test row `routine_id: None` | `routine_id of {...}` left Null | yes |
| A1 | (c) row `line` = object start line | set differs (every line = 1); red before the join assert | yes |
| A1 | (d) `object_stable_id` minted for nested-app files too (`"unfiltered"` guid fallback; no unfiltered guid reaches `file_metrics`, so this simulates it) | `nested object_stable_id` left `"unfiltered/Codeunit/50251"`; A1b also red | yes (also on tempdir) |
| A1b | (a) `zero_assert: Some(own == 0)` without a graph | `zero_assert without a graph` left false | yes |
| A1b | (b) `zero_assert_procedures` counts only `true` | sum left 0, right null | yes |
| A2 | every `X.M()` call counts as an assert | ZeroAssert (2,2,false) vs (0,0,true) | yes |
| A2b | no `asserterror` +1 | OnlyAssertError (0,0,true,false) vs (1,1,false,true) | yes |
| A3 | drop `QualifiedEnum` arm | literal 5 / expression 4; unit `literal_classifier_shapes` red too | yes |
| A3 | drop `Unary` arm | literal 5; unit red at `-5` and `+1.5` | yes |
| A3 | drop `arenearlyequal` | total 8 | yes |
| unit | `Parenthesized` not unwrapped (first try, `e = inner; break`, still unwrapped one level: corrected to not unwrap at all before running) | `(7)` false | yes |
| unit + A3 | drop `DatabaseReference` arm | unit index 7 false; A3 literal 5 | yes |
| A4 | each codeunit's last procedure left out of its sums | computed sums of nested codeunit ([0..] vs [1,2,0,0,1]) | yes |
| A5 | `codeunits` rebuilt through a `HashMap` | 3 runs: A4 codeunit order red 3/3; A5 red 2/3 (one run's hash order happened to be sorted in both binary runs). Recorded honestly; A4's order check is the reliable guard | yes |
| A6 | `is_test` = `is_framework_invocation_attribute` | "ConfirmYes is not a test" | yes |
| A7 | (a) a test's own count = its id's sum | T arm 1 reachable 6 vs 4 | yes |
| A7 | (b) one declaration per id (insert, last wins) | U reachable 2 vs 3 | yes |
| A7 | (c) visited set not seeded | stays green (12/12): **not discriminating**, as the spec predicted; (a) guards the rule | n/a |
| A8 | globals before locals | HiddenAssert (1,1) vs (0,0) | yes |
| A8 | type match AND name `Assert` (name match) | RenamedAssert (0,0) vs (2,2) | yes |
| A8 | `this.` arm removed | ThisAssert (0,0) vs (1,1) | yes |
| A9 | params dropped from lookup | ParamReceiver reachable 0 vs 1 | yes |
| A10 | empty: If else / Case branch body / Case else / While body / Repeat body / For body / Foreach body / Block / AssertError descent (9 separate breaks) | Traversal 11 vs 12, each | yes |
| A10 | an assert's call arguments counted as asserts | Traversal 13 vs 12 | yes |
| A11 | none (documents the flattened path). Under U1's empty-Preproc break the CLI suite stayed 12/12 green: confirms P23, real AL never reaches the `Preproc` arm | — | — |
| U1 | (a) `Preproc` arm empty | 0 vs 2 | yes |
| U1 | (b) first branch only | 1 vs 2 | yes |
| A12 | (a) per-path guard over every call site | ReachesCycle reachable 6 vs 3 | yes |
| A12 | (b) direct callees only | 1 vs 3 | yes |
| A12 | (c) workspace event links added to the call map (what `Graph.fan_out` mixes in) | 4 vs 3 | yes |
| A14 | app-root comparison dropped for declarations | CallsNested (0,1,false) vs (0,0,true) | yes (also on tempdir) |
| A14b | call map keyed by `RoutineNodeId` alone | CallsTwin (0,1) vs (0,0) | yes (also on tempdir) |
| A13 | legacy edit 2 reverted | legacy test `row key order` red (scan entered `test_quality` rows) | yes |

### Goldens

`scripts/check-goldens`: no golden moved. Its only failure was the #160 sweep described
above, fixed by option A (sweep then green). Full gate below.

### Measurement (CDO Test app `/work/src/cdo-baseline/Test`, sibling of `cdo_ws`)

- Builds: master `60434aef` (git archive, own target dir) and the branch, both
  `--profile release-fast`; `cmp`: the binaries DIFFER. No `.alpackages`. Empty HOME.
- JSON time, 1 discarded warm-up each, then 15 interleaved runs each: **median 293 ms
  master, 284 ms branch** (min 223/216, max 773/661; shared host, noisy). The >2x fallback
  does NOT fire.
- Output apart from `test_quality` is identical to master's (`jq -S 'del(.test_quality)'`).
- Counts: test procedures **1844** (exactly as P21 expects); `assert_count` 3316;
  `assert_count_reachable` 3491; zero-assert own count **88** (= P10's grep 88), reachable
  **58**; `asserterror_procedures` 94; 100 test codeunits; no `null` reachable values.
  `are_equal`: total 1528, literal 1044 (**68.3%**), expression 484.
- Differences from the grep numbers, explained: the grep's 1588 AreEqual/AreNotEqual =
  **1528 inside `[Test]` bodies + 60 in helpers** (awk split; `are_equal` is own body only by
  design). The grep's 104 `asserterror` lines: a line-anchored count finds 99 statements in
  94 test procedures (94 = `asserterror_procedures`); the other 5 grep hits are not
  statement starts.
- Hand-check, 5 tests still `zero_assert: true` (spread through the list):
  `CDODuplicateEDocSendGuardTests:1408` (comment says "reaching this line IS the
  assertion"; no assert); `CDOEMailTests:1065` (`SetInfo` only); `CDOEmailEditorPageTests:460`
  (asserts only in a page handler run by the framework: stated limit);
  `CDOOverlapDetectionTests:297` (mocks + `CheckForOverlaps`, relies on "no message
  handler"); `CDOModuleManagerTests:456` (calls only, "no error should be thrown"). All 5
  are real zeros: no direct assert, no walker miss.

### Fallbacks fired

None of the pre-stated ones. The nested-app fixture issue had no pre-stated fallback;
the conductor chose option A (above).

### Gate

`bash scripts/ci-steps task`: green (fmt, clippy-dev, gen-syntax, test 3869/3869, 8 skipped).

### Commits (not pushed)

- `00cd7e5b` analyze --format json: per-[Test] assert counts (test_quality) (#253)
- `a2090e29` docs: CHANGELOG, README and plan for analyze test_quality (#253)

`agentflow check-diff --base origin/master` (origin/master = `60434aef`): no reasons,
before and after each commit.

## Final round 1 fixes

On top of `a2090e29`. Every finding was checked against the code first.

### Decisions

- **F1 (fixed).** Confirmed by a probe: `0D`, `0T`, `0DT`, `20261023D` and `123L` lower
  to `ExprKind::Unknown`; `-123L` is `Unary` over such a node. The node keeps the grammar
  kind in `origin.kind_text` (`date_literal`, `time_literal`, `datetime_literal`,
  `biginteger_literal`; these are the grammar's node types). So the classifier reads that
  kind; it needs no source text and no lowerer change. A signed BigInteger counts too.
  CLI test `f1_date_time_and_biginteger_literals_are_literal` (new last `Main` test
  `DateTimeBigIntegerLiterals`, line 207) and unit test `unknown_literal_kinds_are_literal`.
- **F2 (fixed).** Confirmed: the local `"Assert"""` has IR name `Assert""`, and
  `trim_matches` made it `Assert`. IR names (locals, params, globals, `Identifier`,
  `QuotedIdentifier`) are now compared as they are; only a `Member`'s raw name drops
  exactly one outer quote pair (`unquote_member`), for `this.X` and the method name.
  Unit test `escaped_quote_name_does_not_hide_assert`.
- **CR1 (fixed).** `fill_reachable` (analysis.rs, used by `analyze_project`): with a graph,
  a test without a declaration gets its own count. `null` only without a graph. Unit test
  `join_missed_test_keeps_its_own_count` (join miss stated by hand, as #159's test does).
- **CR4 = G1 (fixed).** The internal id is computed once; `routine_id`'s stable object id
  is `to_stable_object_id(internal)`. `object_stable_id` keeps the internal
  `<guid>/Codeunit/N` form. Same nulls by construction. Guarded by A1 (both fields).
- **CR5 (docs).** CHANGELOG and README: the issue's object key, not a prefix of
  `routine_id` (colon form).
- **CR6 (fixed, with a finding).** Probe: an `#if`-split object HEADER (only the
  `codeunit N "X"` line in two arms) lowers to ONE object already, so it never gave two
  rows. A whole object written in both arms gives two `ObjectDecl`s with the same number;
  that is the shape that gave two rows. Grouping is now by (path, object type, object
  number), with the start line only for an object with no number. Fixture
  `src/Tests/SplitObject.Codeunit.al` (codeunit 50205 in both arms) and CLI test
  `cr6_object_split_across_if_arms_is_one_codeunit_row`. A1/A4 hand lists and totals
  updated for the new rows (Main 15 tests, workspace 21/40/55, `are_equal` 15/12/3).
- **CR8 (fixed).** The body walk and the test rows run only when `analyze_project` gets
  `graph: Some` (JSON, the only format that prints `test_quality`, `main.rs:198`).
- **CR9 (fixed).** `FileRows` is a named struct.
- **CR10 (kept).** The two existing `[Test]` checks are inline expressions, not helpers:
  `root_classification.rs:269-272` (over the model's `attributes_parsed`) and
  `impact_graph/facts.rs:96-99` (inline over `RoutineDecl.attributes`). Nothing to call,
  so `test_quality::is_test` stays.
- **G2 (docs).** CHANGELOG null sentence covers "snapshot could not be built" and the CR1
  rule.
- **G3 (done).** Real A1(d) break run (row below).
- **CR2, CR3 (Limits, no code).** CHANGELOG states both precisely: a routine split across
  object-level `#if` arms is one graph routine (own arm's asserts, union of callees,
  split helper counts both arms); a root call to a nested routine with a colliding id is
  credited to the root's routine (#250).
- **CR7 (refuted).** One walk per test has no measured cost: the CDO Test app JSON run
  is 284 ms vs 293 ms on master (first measurement) and 193 ms vs 183 ms (second, below),
  both far under the 2x line.

### Proofs (Edit, run, revert with Edit; reverts `cmp`-checked against a clean copy)

| test | break | fail output | pass |
|------|-------|-------------|------|
| f1 CLI + unit | drop `date_literal` | are_equal literal 4 / expression 2 (0D, 20261023D); unit `[false,..,false,..]` | yes |
| f1 CLI | drop `time_literal` | literal 5 / expression 1 | yes |
| f1 CLI | drop `datetime_literal` | literal 5 / expression 1 | yes |
| f1 CLI | drop `biginteger_literal` (bare `123L`) | literal 5 / expression 1 | yes |
| f1 CLI | drop the BigInteger clause under `Unary` (`-123L`) | literal 5 / expression 1 | yes |
| F2 unit | `trim_matches('"')` back on both sides of the local/param match | assert_calls 0 vs 1 | yes |
| CR1 unit | test without a declaration stays `None` (`_ => continue`) | "own count, not null": None vs Some(2) | yes |
| CR6 CLI | group by `(path, object_line)` (old rule) | cr6: 2 rows vs 1; A4: 5 codeunit rows vs 4. (A first, wrong-shaped break that also split numbered objects made every row its own codeunit; it was replaced by the exact old rule before recording) | yes |
| A1(d) real (G3) | drop `.filter(|_| roots[..].is_empty())` in `analyze_project` | nested `routine_id` = root-guid id instead of null | yes |
| CR8 | none: no output shows the skipped walk (text/CSV are byte-identical either way; the legacy test pins that). Not discriminating, stated | — | — |
| CR4 | refactor; no new test: A1's `routine_id` and `object_stable_id` checks guard both values | — | — |

After the reverts: analyze CLI 31/31, lib `test_quality::` + `analysis::` 29/29.

### Measurement (re-run: F1 moves the literal split)

Branch rebuilt `release-fast`; same master binary. CDO Test app: 1844 tests, 3316 own,
3491 reachable, 88/58 zero-assert, 94 asserterror, 100 codeunit rows (CR6 merges none on
CDO), `are_equal` 1528 = **1096 literal (71.7%)** + 432 expression (was 1044/484: 52 date,
time, datetime or BigInteger expected values moved to literal). Output apart from
`test_quality` identical to master. Timing, 15 interleaved runs each: median **183 ms
master, 193 ms branch** (min 172/181, max 217/220; quieter host than the first run).

### Gates

- `bash scripts/ci-steps task`: green, 3874/3874 (8 skipped).
- `CHECK_GOLDENS_FORCE=1 scripts/check-goldens`: green, 1025/1025 (4 skipped); no golden
  moved, and the #160 sweep and cone oracles pass with the new `SplitObject` file.

### Commit and check-diff

- `c1f6129b` analyze test_quality: final round 1 fixes (#253)
- `check-diff --base origin/master`: origin/master moved to `adde813c` (#254 merged);
  reasons `protected-path:.agent/issue-254/{findings.json,ledger.md}`, which are master's
  files, not this branch's. `check-diff --base 60434aef`: **no reasons**. The branch
  changes 0 `.agent/` files.
- **Rebase needed (not done here: #254's files are hands-off).** `git merge-tree
  origin/master HEAD` conflicts in `CHANGELOG.md` (both add the top `Added` entry; keep
  both) and `src/engine/l4/capability_cone.rs` `FIXTURE_ROOTS`. Master has
  `("fixtures", 38)` (#254 added 2 app dirs) and this branch has 37 (+1), so the merged
  value must be **39**. `README.md` and `tests/cli/main.rs` auto-merge.

## Final panel (conductor)

- Final round 1 (head a2090e29): sol F1 (important, date/time/BigInteger literals), F2; opus ship (G1-G3 minor); built-in code-review CR1-CR10. Conductor: CR5 keeps the operator-requested `<appId>/Codeunit/N` form (documented); CR1 graph-but-no-declaration falls back to the own count; CR2/CR3 stated Limits (CR3 → #250); CR7 refuted by measurement; CR10 refuted (no reusable helper). Fixed in c1f6129b.
- Master moved to adde813c (#254). Conductor rebased: CHANGELOG (both `Added` entries kept) and `FIXTURE_ROOTS` ("fixtures", 39). Task gate green on the rebased head (77.9 s). Pushed with --force-with-lease (53426f83).
- Final round 2 (head 53426f83): both accepted all 20 ids; sol F3 (minor, stale doc comment), fixed by the conductor in 8a3ec7b1.
- Final round 3 (head 8a3ec7b1): both accepted F3; both ship. Register: 39 entries, all accepted by both, none open. final_rounds remaining 2.
- Step 11 gates on 8a3ec7b1 (base adde813c, supervised, run 20261023-192451-fe0173): check-goldens-coverage exit 0 (0.0 s); ci-steps-all exit 0 (415.5 s); check-goldens exit 0 (0.1 s, tree-stamp skip); cdo-gate exit 0 (465.9 s).
