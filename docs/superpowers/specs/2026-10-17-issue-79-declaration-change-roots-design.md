# Issue #79: digest maps object-level declaration changes to routines (design, revision 5)

Status: spec panel, revision 5. Revisions 4 and 5 are targeted edits on revision 3's design (see the register).

Evidence:
- **Investigation:** `.agent/issue-79/investigation.md`.
- **Resolver-reuse study:** `.agent/issue-79/resolver-reuse.md`.

Both were run on master `a744cd61`.

Revision 3 replaces revision 2's hand-written binder (D3) with a **reference-collection pass inside the program resolver**. The pass reuses the resolver's own scope lookup and receiver typing. Spec rounds 1 and 2 kept finding binding rules that the binder missed: the page-extension `SourceTable`, `Record 18` vs `Record "18"`, dataitem names, declared vs implicit `Rec`, named returns, `this.G`, and protected base globals. The resolver already implements every one of them (`receiver.rs`, cited per rule below). A second copy would drift.

## Goal

Today a `--diff` hunk that changes a global-variable declaration or a table-field declaration fails with `hunk-outside-routines` (exit 2). After this change, it roots the routines the change can affect:

- **Global variable G of object O.** Every workspace routine whose body references G. A reference is:
  - a name that binds to G in the routine's lexical scope;
  - `this.G`;
  - from an extension object, a `protected var` G of its base.
- **Table field F, physical table T.** For a tableextension field, T is the extended table. The rooted routines are:
  - every workspace routine whose body reads or writes F of T, as the resolver types it;
  - F's own member triggers, in T's declaring object and in `modify(F)` blocks of T's tableextensions.

## Non-goals (stated limits; ★ = becomes a discovery)

**Hunks that still produce `hunk-outside-routines`:**
- the object header;
- object properties;
- keys and field groups;
- page layout controls and their `SourceExpr`;
- actions;
- report dataitems and columns;
- query columns;
- enum values;
- tableextension `modify(...)` headers outside their triggers;
- `var` keyword lines;
- `#if` directive lines;
- comments and blank lines.

**Excluded from declarations:** XmlPort text nodes. Each one spans a whole element subtree.

**Dependants that are not routine bodies.** These are not followed:
- a FlowField `CalcFormula` over F;
- `TableRelation`;
- a page `field(…; Rec.F)` source expression;
- report columns.

**Event subscribers keyed by a field name** (`[EventSubscriber(…, 'OnAfterValidateEvent', '<F>', …)]`) are not rooted by a change to F ★. The attribute names the field as text; it is not a body reference.

**Honest dynamic access, not followed:** `RecordRef`/`FieldRef`/`Variant`, `FieldNo` with an integer literal.

**Bare field names in a `TableNo` codeunit ★.** A name with no `Rec.` prefix in such a codeunit is not rooted. The resolver's Step 3a does not type them either: its kind gate excludes Codeunit (`receiver.rs:984-997`). This keeps one engine. Explicit `Rec.F` in a `TableNo` codeunit IS covered, because R3 types `Rec` through `infer_implicit_rec`'s Codeunit branch (`2776-2781`).

**Not reported:** receivers that the resolver types `Unknown` (call-result chains outside its return tables, `Variant`, receivers it declines inside `with`). They are COUNTED in `untyped_member_reads`, not reported per routine.

**Other limits:**
- `Unknown` IR nodes and parse-incomplete routines.
- A split `#if` field header resolves to its first arm.
- `--file` and `--routine` are unchanged.

## Acceptance matrix

All root-set assertions compare `(objectDisplay, display, stableId)` tuples. Every test asserts the EXACT `changedRootsDiagnostics` set. The tests are CLI tests in `tests/cli/cli_digest_changed_spans.rs` (temp-dir workspaces) unless marked "unit".

| # | Case | Proof |
|---|---|---|
| A1 | Field declaration (the issue's reproducer) | `d8_workspace`, a diff on line 29 `field(50; "Status Posted"; Boolean)`: exit 0, roots == {`D8PostingChain.PostSalesDoc`}, `rootedBy` == [field `"Sales Header"."Status Posted"`], no diagnostics. |
| A1b | Field only as a record-method argument | Line 39 `"Sales Line"."Status Posted"`: `PostSalesDoc` is a root only through `Line.ModifyAll("Status Posted", …)` (rule R4). |
| A1c | Cross-file reader | Line 48 `"Audit Entry"."Doc No."` roots `HandlePosted` (`src/subscriber.al`). |
| A1d | Unreferenced field | Line 47 `"Audit Entry"."Entry No."`: exit 2, diagnostics == [`declaration-unreferenced`]. |
| A1e | Declaration-only file | `src/Log.Table.al` is a table with fields and no triggers; a codeunit in another file writes `L."Msg"`. The diff on `Msg` roots that routine, and the file is NOT `diff-file-unmatched`. A pure rename of that file with no hunks touches every declaration in it, mirroring the routine rule. |
| A2 | Global declaration (the issue's acceptance) | `codeunit 50108` has the global `PostCount: Integer`:<ul><li>`PostSalesDoc` reads it plainly.</li><li>`QuotedUse` reads it as `"PostCount"`.</li><li>`ThisUse` declares a local `PostCount` and reads `this.PostCount`.</li><li>`Shadowed` has a local `PostCount` and reads only that local.</li><li>`NoRef` does not use it.</li></ul>There is also a global `GCu: Codeunit X`, which `ParenlessUse` uses as `GCu.Run;`, and a global `GHeader: Record "Sales Header"`, which `ThisChain` uses as `this.GHeader."Status Posted" := true`.<br>Results:<ul><li>Diff on the `PostCount` line: roots == {`PostSalesDoc`, `QuotedUse`, `ThisUse`}.</li><li>Diff on the `GCu` line: roots == {`ParenlessUse`}.</li><li>Diff on the `Status Posted` field: roots include `ThisChain`.</li></ul> |
| A2b | Table global | `table 36` has the global `InsertCount`, used by `OnInsert` and by the field trigger `"Status Posted".OnValidate`. Both are roots. |
| A2c | Unreferenced global | Gives `declaration-unreferenced` and no root. |
| A2d | Named return | `procedure NR() PostCount: Integer` assigns `PostCount := 1`; it is NOT rooted by the global. `procedure NRRec() H: Record "Sales Header"` sets `H."Status Posted"`; it IS rooted by the field. |
| A2e | Non-ASCII names | The global `Ændring` is referenced as `ændring := 1`, and that routine is rooted. |
| A2f | Protected global from an extension | `table 36` declares `protected var PG: Integer`. A tableextension trigger reads `PG`, and a diff on the `PG` line roots it. |
| A3 | Implicit Rec | In `table 36`, `OnModify` has ONLY a bare `"Status Posted" := false`, and `OnDelete` has ONLY `if xRec."Status Posted" then`. Both are roots. A page with `SourceTable = "Sales Header"` whose action reads `Rec."Status Posted"` is a root. A routine of table 36 that calls `Sink.Accept("Status Posted")` on a codeunit `Sink` is also a root (a bare field passed as a value). |
| A3b | The field's own trigger | The field has its own `OnValidate`, whose body does NOT reference the field.<ul><li>A header-line diff roots `OnValidate` (F3) and its readers.</li><li>A diff inside the body roots only `OnValidate`, with no `rootedBy`.</li><li>On a one-line `field(…) { trigger OnValidate() begin … end; }`, a diff to that line roots `OnValidate` directly AND fans out.</li><li>A field property deleted right after the trigger's `end;` (a deletion gap) touches the field and is not `hunk-outside-routines`.</li></ul> |
| A3c | Tableextension, in its own file | `src/SalesExt.TableExt.al` holds `tableextension 50110 extends "Sales Header"`. It adds `"Ext Flag"` and has `modify("Status Posted") { trigger OnAfterValidate() … }`. A codeunit with `H: Record "Sales Header"` writes `H."Ext Flag"`.<ul><li>The `"Ext Flag"` diff roots that routine, with `rootedBy[0].declaringObject` = the tableextension.</li><li>The `"Status Posted"` diff also roots the `modify` trigger.</li></ul> |
| A3d | Wrong-table guards | Tables A and B both have a field `Code`. Table A's trigger does `RB.SetRange(Code, 'x')` with `RB: Record B`, and nothing else.<ul><li>A's `Code` diff does NOT root it, because `Code` sits in a field-selector position typed to B (rule R4).</li><li>B's `Code` diff DOES root it.</li><li>A codeunit with `R18: Record 18` and a codeunit with `RQ: Record "18"` (a table named `18`, numbered differently) each read `.Code`. Each one is rooted only by its own table's diff.</li></ul> |
| A3e | Dataitem name | Report `R` has dataitem `SalesHeader` (table `"Sales Header"`). A report procedure reads `SalesHeader."Status Posted"`, and the dataitem's own `OnAfterGetRecord` trigger reads the bare `"Status Posted"` (Step 3a's report branch). The field diff roots both. |
| A3f | Unresolved table | With `--single-app`, the tableextension's base sits in a missing dependency, so its table is unresolved. The `"Ext Flag"` diff still roots, by name, both a direct reader (`H."Ext Flag" := true`) and a method-argument-only reader (`H.ModifyAll("Ext Flag", true)`). Each `rootedBy` item has no `stableId` and has `match: "name"`. |
| A2g | `#if` global with different types | A global `G` is declared `Integer` in one `#if` arm and `BigInteger` in the other, and a routine does `G := 1`. The `G` diff roots that routine, and the resolver's own call graph is unchanged (A7). |
| A3g | `with` in the same routine | A table routine reads the bare `F` before a `with Other do Message('x');` and again after it. The `F` diff roots that routine. |
| A4 | Mixed hunks | The existing A1e/A1f pins are rewritten to these semantics. A key or property line stays `hunk-outside-routines`. |
| A5 | Output contract | `schemaVersion == "1.7.0"`. `rootedBy` is present only when non-empty. `#if` duplicate declarations give one item, anchored at the first declaration in document order. The human output matches the D5 lines exactly, including the no-entries branch and a value with spaces and quotes. |
| A6 | Opt-in (unit + integration) | A model assembled through the `alsem analyze` entry, in both single-app and cross-app, has `routine_references == None` everywhere. The digest's model has `Some` on its workspace routines. |
| A7 | Resolution unchanged | The resolver refactors (D3 helpers) leave `scripts/check-goldens` green and the CDO `--program-call-graph-stats` JSON byte-identical between master and the branch. |
| A8 | Reference pass (unit) | Table-driven over small workspaces. Each rule R1-R5 gets one positive case and one negative case, and the `untyped_member_reads` count is pinned. |

### Discrimination

Each break is made with Edit, run, and reverted. Each must make the listed test fail.

| # | Break | Must fail |
|---|---|---|
| 1 | Drop the declaration pass | A1, A2 |
| 2 | Drop rule R4 | A1b |
| 3 | Treat every scope binding as Global | A2 `Shadowed` |
| 4 | Drop `QuotedIdentifier` from R1 | A2 `QuotedUse` |
| 5 | The walk does not descend into a callee Member's object expression | A2 `ParenlessUse` |
| 6 | Drop R2 (`this.`) | A2 `ThisUse` |
| 6b | R3 types a `this.G` receiver as `Unknown` (skip the `this` arm of the compound typing) | A2 `ThisChain` |
| 6c | `caller_scope_binding` treats a global-tier duplicate as not-found | A2g |
| 6d | Use the resolver's whole-routine `WithCtx::state()` instead of the pass's own depth | A3g |
| 6e | R4 sends `Record{None}` selectors through R1 | A3f method-argument reader |
| 6f | Skip member normalization | A8 quoted member |
| 7 | Treat a named-return binding as not-found | A2d |
| 8 | Fold with ASCII lowercase | A2e |
| 9 | Drop the protected-base branch | A2f |
| 10 | Drop the implicit-field helper | A3 `OnModify`, `Sink` |
| 11 | Drop implicit `xRec` typing | A3 `OnDelete` |
| 12 | Drop F3 | A3b |
| 13 | Routine always wins | A3b one-liner |
| 14 | Deletion gap tested by line | A3b gap |
| 15 | Match against the extension stub, or drop F3 for extensions | A3c |
| 16 | R4 also records field-selector arguments as implicit fields | A3d |
| 17 | Fold the `ObjectRef::Id`/`::Name` distinction (compare by name) | A3d `18` |
| 18 | Skip Step 2b (dataitem and table-element names) in receiver typing | A3e procedure reader |
| 18b | `implicit_table(R)` returns `None` for Report/ReportExtension | A3e trigger's bare field |
| 18c | R3 passes `bare_ctx = None` | A8's receiver whose typing needs `bare_ctx`: a Step 5 compound receiver, e.g. a call-result chain `GetHeader()."Status Posted"`, where the resolver types `GetHeader`'s return table |
| 19 | Drop the unresolved name fallback | A3f |
| 20 | Keep the old unmatched-file guard | A1e |
| 21 | No `declaration-unreferenced` | A1d, A2c |
| 22 | Build the pass unconditionally | A6 |
| 23 | Drop `rootedBy` or a human line | A5 |

## Design

### D1. Lowerer: `FieldDecl.origin`

`FieldDecl` (`crates/al-syntax/src/ir/decl.rs:72-85`) gains `origin: Origin`. It is set in `lower_field` (`lower/mod.rs:547-614`), the only site that constructs it, from `origin_of(node)`. The node is the `field_declaration`, or the `preproc_split_table_field[_open]` node for a split `#if` header. Spans may NEST (a split field contains the leading complete fields), so D4 scans declarations linearly. The change is reviewed by `lowerer-reviewer`.

### D2. Model: member declarations, keyed like the resolver

`ModelObject` gains `member_declarations: Vec<ModelMemberDeclaration>`. It is built where `declaration_only_routines` is built (`workspace.rs:897-954`):

```rust
pub struct ModelMemberDeclaration {
    pub kind: MemberDeclKind,              // Global { name, protected: bool } | Field { number: i64, name }  (original case)
    pub source_anchor: PAnchor,            // VarDecl.origin / FieldDecl.origin
    pub physical_table: Option<Box<str>>,  // Field only: model object id (encode_object_id) of T, computed by the
                                           // resolver's own helpers (Table → itself; TableExtension →
                                           // resolve_tableext_base_table, receiver.rs:3004); None when unresolved
}
```

- **Globals:** all of `ObjectDecl.globals`, including both `#if` arms and protected globals. XmlPort text nodes are excluded.
- **Fields:** `ObjectDecl.fields`, for Table and TableExtension.
- `member_declarations` is not serialized. It is set at every `ModelObject` literal.

### D3. The reference-collection pass (`src/program/resolve/references.rs`), opt-in

```text
collect_workspace_references(ctx: &ProgramContext)
    -> HashMap<(virtual_path, object index, routine index), RoutineReferences>
```

**Setup.**
- It builds `ResolveIndex::build(ctx.graph())` and `ctx.decl_surface()` once.
- It walks the same workspace files and object map as phase 1 (`full.rs:1220-1244`), in parallel, on `big_stack_pool()`.
- The walk is COMPLETE. It covers every `StmtKind` and every lowered `#if` arm, using `walk_stmt_v2`'s statement coverage (`extract.rs:821-944`). It covers every expression through `ExprKind::children()`.
- The `WithState` passed to R3/R5 receiver typing comes from the pass's OWN lexical `with` depth (see "`with` tracking for the pass"): the no-`with`-in-effect state (`NoWithProven`) at depth 0, the inside-`with` state within a body. The resolver's `WithCtx` is not reused and not made public.

```rust
pub struct RoutineReferences {
    pub globals: Box<[(Box<str>, Box<str>)]>,  // (owning object's model id, folded name)
    pub fields: Box<[(Box<str>, Box<str>)]>,   // (physical table's model id, folded field name)
    pub unresolved_fields: Box<[Box<str>]>,    // folded field names read on Record{table: None}
    pub untyped_member_reads: u32,             // Member reads whose receiver typed Unknown (a gap count)
}
```

- Every list is sorted and deduplicated.
- **Model ids** are built with `encode_object_id(app_guid, type, number)`. The app guid comes from the SAME source `project_ir` uses for `ModelObject.id` (the snapshot's app identity, `workspace.rs:759`), not from `graph.apps`, whose doc says the guid is empty for dependencies (`node.rs:13-14`). A8 asserts that the pass's object id equals `ModelObject.id` for every workspace object, and equals the model's dependency-object id for a dependency table in cross-app mode.
- Nothing is serialized. `ModelRoutine` is not `Serialize`, and the struct carries no serde attribute.
- **One name normalization**, the RESOLVER'S OWN key representation, used on both sides of every comparison:
  - **Rule:** strip the outer quotes only (`receiver::unquote_identifier`, `receiver.rs:3500-3508`), KEEP an inner escaped `""` as is, then apply `fold_identifier`.
  - **Why:** this is exactly how fields are indexed. The lowerer keeps inner escapes (`lower/mod.rs:3320-3328`), `node_extract.rs:698-707` folds that text, and `field_in_table` and `fields_named_in_table` compare against it (`index.rs:819-822, 837-840`).
  - **Where it applies:** the outer-quote strip applies ONLY to RAW token text: `Member.member`, which may still carry its outer quotes (`ir/expr.rs:16-21`) and is always normalized before any lookup, and the raw text of selector arguments. Names the IR already outer-stripped get `fold_identifier` ONLY: a `QuotedIdentifier(name)` payload, and `VarDecl.name`/`FieldDecl.name` from `ident_text`. A second strip would corrupt a name whose logical text starts and ends with an escaped quote. For example, the field `"""X"""` has IR name `""X""`. A8 includes that shape. Every key the digest compares therefore has the same shape as the graph index and the scope declarations.
  - **Collapsing `""` is NOT done**, anywhere.
  - **Test:** A8's escaped-quote case declares a field `"A""B"`, reads it as `R."A""B"`, and asserts that the root goes through the full path: member, then `fields_named_in_table`, then the D2 key.

**Rules.** Each rule is a call into the resolver.

| Rule | Shape | Resolver call | Recorded |
|---|---|---|---|
| R1 | Bare `Identifier`/`QuotedIdentifier`, in any position except the callee of a `Call`. This includes the object of a callee Member: `GCu` in `GCu.Run;` is reached through the child walk. | `caller_scope_binding` (new; `caller_scope_symbol`, `receiver.rs:3607-3683`, becomes its projection). Order: param, local, named return, global. It reports WHICH tier bound the name, and which tier (if any) is a `MalformedDuplicate`. | `Global` → `(own object, name)`. If ONLY the global tier is a duplicate (both `#if` arms declare G with different types) and no param, local or return binds the name, it still records `(own object, name)`. The resolver's own projection keeps declining exactly as today. Param/Local/Return → nothing. NotFound → if a base `protected var` has the name (`base_protected_var_owner`, new; `base_protected_var_type` is built on it, `receiver.rs:1335-1355`) → `(base object, name)`. Else, OUTSIDE every active `with` body → `implicit_table(R)` (new, extracted from Step 3a, `receiver.rs:984-1006`; it returns ONLY the implicit table id per object kind, with the same kind gate) → then `fields_named_in_table(T, name)` → `(T, name)` for each candidate. Inside a `with` body → R5. |
| R2 | `this.X` (`is_this_identifier`, `receiver.rs:2361-2366`) | object globals (`infer_this_member`, `2536-2551`) | `(own object, X)`, whatever locals shadow it |
| R3 | `Member{object, m}` as a value, an assignment target, or a chain receiver; not a callee | `infer_receiver_type_for_expr(object, …, Some((surface, with_state)))` (made `pub(crate)`, `receiver.rs:1443-1720`). This covers declared vs implicit `Rec`/`xRec` (Steps 2/3b, `infer_implicit_rec` `2706-2810`), dataitem and table-element names (Step 2b, `886-895`), `ObjectRef::Id` vs `::Name` (`528-544`), named returns, `this.G` chains, and protected base globals. | `Record{Some(t)}` and `m` names a field visible on t (`fields_named_in_table`, new: `field_in_table`'s search without its uniqueness decline, `index.rs:805-874`) → `(t, m)`. `Record{None}` → `unresolved_fields += m`. `Unknown` → `untyped_member_reads += 1` when `m` is a field name anywhere in the workspace. Then recurse into `object`. |
| R4 | `Call{function: Member{recv, method}, args}` where `recv` types `Record{Some(t)}` or `Record{None}` | the same typing as R3 | Field-selector positions → `(t, arg)` when the arg names a visible field of t. For `Record{None}`, they go to `unresolved_fields` instead; they never go through R1, because R1 would bind them to the CALLER's own table. They are taken from the resolver's record-method catalog if it encodes field-parameter positions; otherwise from this fixed table: arg 0 of `SetRange`, `SetFilter`, `TestField`, `Validate`, `FieldNo`, `FieldCaption`, `FieldError`, `FieldActive`, `ModifyAll`, `CopyFilter`, `GetFilter`, `GetRangeMin`, `GetRangeMax`; every arg of `CalcFields`, `CalcSums`, `SetCurrentKey`, `SetLoadFields`, `AddLoadFields`. Selector positions are NOT also run through R1. Non-selector arguments go through R1 normally. `recv` itself goes through R3. |
| R5 | `with R do body` | R typed through `infer_receiver_type` | A body name that R1 leaves unbound is recorded as `(t_R, name)` when `R` types `Record{Some(t_R)}` and the field exists, AND as the implicit field (R1 without the `NoWithProven` gate). This is conservative. |

**`with` tracking for the pass.** The resolver's `WithCtx::state()` returns `Unknown` for the WHOLE routine if any real `with` token appears anywhere in it (`extract.rs:493-537`). That signal is right for the resolver, which stays unchanged. The pass needs finer tracking: it keeps its own lexical `with` depth during the walk. At depth 0 a name is treated as outside any `with` (R1's implicit-field branch applies). Inside a body, R5 applies. A8 pins a field read before and after a `with` in the same routine.

**Recall over precision.** The pass deliberately does NOT use the resolver's fail-closed last-hop declines (`field_in_table`'s uniqueness decline; the `table_scope_has_routine` parens-less guard, `receiver.rs:2004`, `1013-1024`). For digest roots, an extra root is safe and a missed root is not. The resolver's own resolution keeps both declines.

**PageExtension `SourceTable` split.** `implicit_rec_table_id` (`resolver.rs:1430-1445`) uses the base page's `SourceTable`, while `infer_implicit_rec` (`receiver.rs:2747-2755`) uses the extension's own first. The fix makes `implicit_rec_table_id` delegate to the same per-kind rule (and `implicit_table(R)` is that rule). The implementer first checks whether alc accepts `SourceTable` on a page extension:
- **If alc accepts it:** A3d gains a page-extension case (its own `SourceTable` = B over a base page on A, a bare `Code` read is rooted by B.`Code` only), with its own discrimination break.
- **If alc rejects it:** the two rules are identical on valid code, and A7 (byte-identical) is the pin.

If a golden or CDO number moves, it is triaged before anything is blessed.

**Resolver refactors keep resolution identical.** This covers `caller_scope_binding`, `implicit_table`, `base_protected_var_owner`, `fields_named_in_table`, and the visibility of `infer_receiver_type_for_expr`. A7 is the gate.

**Opt-in and plumbing.** One explicit flag goes through BOTH model-assembly entries:
- `program_calls::assemble_and_resolve_workspace_program`, single-app (`program_calls.rs:713-728`);
- `assemble_and_resolve_cross_app_program`, cross-app (`816-856`; it is shared with analyze, events and policy check through `build_cross_app_base` / `AnalysisTarget::build`, `gate/run.rs:191-213`).

The digest passes `true`; every other caller passes `false` and pays nothing. When the flag is set, `collect_workspace_references(&ctx)` runs before model assembly. `project_ir` (`workspace.rs:759`) attaches each routine's entry by `(virtual_path, object index, routine index)`. This is the same key and the same precedent as the per-file `ParenlessCalls` side table (`full.rs:123`, `workspace.rs:1700-1722`). Dependency rows get `None`.

### D4. Digest: declaration spans and resolution

**Index.** `resolve_changed_roots` (`digest_cli.rs:376-520`) builds a `DeclarationSpanIndex` beside `RoutineSpanIndex`:
- it holds `ws:` units only, with each declaration's 0-based `[start, end]`;
- it is scanned linearly, because spans nest;
- a diff file is `diff-file-unmatched` only when it has NEITHER kind of span (this replaces the guard at `:466-472`);
- a pure rename with no hunks touches every declaration in the file, mirroring the routine rule at `:477-482`.

**Per change.** This uses `touched_by`'s semantics (`:250-259`).
- A touched routine span → that routine is touched directly.
- A declaration D is touched when the change touches D's span AND one of these holds:
  - (a) an added or changed line is D's first line;
  - (b) the line lies inside no routine span;
  - (c) for a deletion gap after line `l`, the gap between `l` and `l+1` is not interior to a routine span: the routine does not contain both `l` and `l+1`.

A hunk is `hunk-outside-routines` only when none of its changes touched a routine or a declaration.

**Resolution.** The candidates are workspace routines with `routine_references`.

**Global G** (declaring object O): every R with `(O, fold(G)) ∈ R.globals`. This includes extension routines when G is protected (R1's base branch).

**Field F** (declaring object D, physical table T = `physical_table`, name f):
- **F1.** `(T, fold(f)) ∈ R.fields`.
- **F1′.** When `T` is `None`, or `fold(f) ∈ R.unresolved_fields`, match by name and mark the item `match: "name"`.
- **F3.** R is a member trigger of F: `R.enclosing_member` folds to f, and R's object is D or a tableextension of T (`modify(f)` triggers).

**Roots.** Each resolved routine goes into `hits` with all of its own spans (`spans.of_routine(id)`, the precedent at `:426-429`), and the digest records `routine id → set of RootedBy`.

**Unreferenced.** A touched declaration with zero routines gives one `declaration-unreferenced` per (hunk, declaration) and adds no root. The exit rule is unchanged: no roots → 2.

### D5. Output contract (digest `schemaVersion` 1.6.0 → 1.7.0)

**`entries[].routine.rootedBy`.** It is present only when non-empty. Items are sorted by `(kind, declaringObject, folded name)` and deduplicated on that key. For `#if` duplicates, the kept item is anchored at the FIRST declaration in document order.

```json
"rootedBy": [
  {"kind": "field", "declaringObject": "<model id of the declaring TABLE or TABLEEXTENSION>",
   "name": "Status Posted", "display": "\"Sales Header\".\"Status Posted\"",
   "stableId": "<to_stable_field_id(appGuid, physical table number, field number); omitted when T is unresolved>",
   "match": "name",   // present only for an F1′ name match
   "anchor": {"sourceKind": "source", "file": "ws:src/posting.al", "line": 28, "column": 8}},
  {"kind": "global", "declaringObject": "<model id>", "name": "PostCount",
   "display": "D8PostingChain.PostCount", "anchor": {…}}
]
```

- Globals carry no `stableId`, so no new id shape is introduced.
- Names keep their original case.

**`changedRootsDiagnostics` gains:**

```json
{"kind": "declaration-unreferenced", "file": "src/posting.al", "startLine": 47, "endLine": 47,
 "declaration": {"kind": "field", "declaringObject": "…", "name": "Entry No.", "display": "\"Audit Entry\".\"Entry No.\""}}
```

**Human format** (`format_digest_human`, `digest_cli.rs:1438+`):
- **Per entry:** one line per `rootedBy` item, right after the entry header: `  rooted by: <kind> <display>`. A name-matched item appends ` (name match)`.
- **Changed-roots diagnostics:** every kind is rendered, one line each. The lines come after any existing preamble line (the dependencies line) and before the first entry. In the no-entries branch, they come before `digest: no changed roots resolved`.
  - Format: `changed-roots: <kind> <key>=<value> …`.
  - Keys appear in the canonical serializer's order (`format_json.rs:53-71`), without the top-level `kind`.
  - Nested objects are flattened with dotted keys, and keep their own `kind`.
  - Strings are printed as JSON string literals; numbers are printed bare; arrays are joined with ` | `.
  - Example: `changed-roots: declaration-unreferenced declaration.declaringObject="…" declaration.display="\"Audit Entry\".\"Entry No.\"" declaration.kind="field" declaration.name="Entry No." endLine=47 file="src/posting.al" startLine=47`.

**Pins.** The pins in `cli_digest_changed_spans.rs`, `cli_digest_query_dependencies.rs` and `cli_digest_temp_state.rs` move to "1.7.0".

**Relation to #81.** #81 (blocked) should add `{"kind": "interface-method", …}` items to this list.

## False-positive and failure shapes

| Shape | Outcome | Why |
|---|---|---|
| A same-named routine exists (a parens-less call might be meant) | extra root | recall over precision; stated |
| Several table extensions declare the same field name | root for each candidate | `fields_named_in_table` keeps all candidates; the resolver's uniqueness decline is skipped deliberately |
| A name inside a `with` body | may over-root | conservative; `with` is deprecated |
| A multi-name `A, B` declaration | both touched | they share one span |
| An unresolved table | name match, flagged `match: "name"` | honest |
| A receiver typed `Unknown` | counted, not rooted | `untyped_member_reads`; stated |
| `RecordRef`/`FieldRef` | under-root | honest-dynamic |
| Memory and time | digest only | opt-in; measured |

## Measurement plan

- **Resolution unchanged (A7).**
  - `scripts/check-goldens` stays green.
  - `cargo test -p al-sem --lib` passes.
  - The CDO `aldump --program-call-graph-stats` JSON is byte-identical between master and the branch (cdo profile).
- **Goldens** (`--test cli`, triaged by `golden-diff-triager` before any regen):
  - the `schemaVersion` line in the 21 `tests/cli-b-goldens/digest/*.json`;
  - every `*.human.txt` whose JSON twin has a non-empty `changedRootsDiagnostics` gains the `changed-roots:` lines. The implementer enumerates the exact list first.

  No root moves.
- **CDO**, master vs the branch:
  - `cdo-gate`;
  - peak RSS of `alsem analyze`, which must be within noise (opt-in);
  - for a `--diff` digest of one CDO table file: peak RSS, time, and the pass's own wall time;
  - for 3 real field hunks and 2 global hunks: the roots added, and the `untyped_member_reads` total, sampled against the source.
- **Lowerer:** a `lowerer-reviewer` pass on D1.
