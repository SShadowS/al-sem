# Intent evidence for agents: design (rev 3, 2026-10-30)

**Rev 3 changes** (after an independent review against master `50c981e5`):

- Label text is no longer "dropped": G52 (`318045b7`) added `VarDecl::label`
  (`LabelValue { text, locked }`). Step 1 now **extends** that struct instead of adding
  a second one (4.1).
- The `intent_text` build-profile field, `LowerOptions`, `parse_with` and the
  `DepKey` / `PackedFile` changes are **gone**. Text is a plain IR fact every profile
  keeps. Its memory cost is measured after step 2 and recorded (4.1b). Rev 2 had
  designed a gate for a cost nobody had seen.
- Rules are built **at model assembly** from the IR. After assembly the model keeps
  no full conditions, so a detector-context fill could not build them. A `rules`
  need in `ViewNeeds` gates them. `substrate::RULES` now builds only the per-field
  writer index that d65 reads (4.1b, 4.2).
- `RULES` implies `ORDERING_FACTS`, because "before the first write" needs statement
  order, which the conditionality lattice does not give (4.3).
- `Error(ErrorInfo)` and an own-object `procedure Error` "may not stop". `ConfirmAbort`
  is renamed `ConfirmExit`, because `exit` after a `Confirm` is a silent return, not a
  rejection (4.2).
- The tests guarding d65's declaration are named honestly (6). Common shapes the
  condition builder misses are stated as limits or atoms (4.2, 5).
- Every code citation was re-checked against `50c981e5`.

Rev 2 dropped rev 1's `BuildProfile::INTENT` preset, which was described as a
"superset of `FULL`" and so broke the compact-graph spec §4 rule that `FULL` keeps
everything.

## 1. Goal

An agent (an LLM editing AL code) breaks business logic when it cannot see *why* code
is shaped the way it is. al-sem already tells it *what* code does: tables written,
commits, events, callers. This spec adds the other half: the evidence a human left
behind about intent, extracted precisely and linked to the code it governs.

The engine does **not** guess intent. It gathers evidence, each item with a file and
line, and hands it to the agent. The agent proposes the intent and the engine checks
it against the code. One new check also comes out of this: code that skips a rule the
rest of the code follows.

**Done means:**

1. `alsem explain <ws> --routine <name>` prints an evidence card (4.4) for one
   routine, as terminal text or JSON.
2. `alsem explain <ws> --diff <patch>` prints one card per changed routine. This is
   the agent's entry point: "here is what you are about to touch, and the rules on it".
3. `alsem explain <ws> --table <T> --field <F>` lists every writer of that field with
   the rules each one applies. This is the view an agent needs before it adds a new
   writer, and it needs no routine name.
4. Each check in a card ("rejects when X") carries its condition, its message text
   (with label text resolved) and a source anchor.
5. A routine with no checks still gets a full card: `rules.own` is empty and the card
   says so (`"own": []`), and its effects, callers, inherited rules and gaps are
   listed. An empty list means "the engine looked and found none", never "not built".
6. A new opt-in detector, `d65-guard-bypass`, reports writes that skip a check that
   most other writers of the same field apply. It goes through the normal triage on
   CDO before it may run by default.
7. Everything the engine could not see (symbol-only dependencies, unparsed bodies,
   `unknown` edges in the cone) appears in the card as a stated gap. It is never
   left out silently.

**Not in scope:** calling an LLM from the engine; reading git history or work items;
natural-language summaries written by the engine. The engine only emits facts.

## 2. Terms

- **check**: a statement that stops the success path when a condition holds: an
  `Error(...)` call, `Rec.TestField(...)`, `Rec.FieldError(...)`. A `Confirm` whose
  `false` result leads to `Error` is a check too. A `Confirm` whose `false` result
  leads to `exit` is recorded as a `ConfirmExit` rule: it ends the routine silently,
  without an error.
- **rule**: a check together with the condition under which it fires and its message.
  Written "rejects when C: M".
- **condition atom**: one part of a condition after normalising it, for example
  `"Sales Header".Status <> Released` or `param PostingPreview = true`.
- **evidence card**: everything this spec extracts for one routine, in one record.
- **writer**: a routine that writes a given table field (directly, or via `Validate`,
  `Modify` after assignment, `ModifyAll`).
- **gap**: a reason the card may be incomplete.
- **lowerer**: the code in `crates/al-syntax` that turns the parse tree into the
  engine's own syntax data (the IR).

## 3. Where intent lives in AL code, and what the engine keeps today

Checked against master `50c981e5`.

| Evidence | Why it carries intent | Kept today? | Where |
|---|---|---|---|
| `Error` / `TestField` / `FieldError` sites | They are the business rules | Site only. `Error` is an `error-call` operation site and becomes an `error-throw` capability fact with no message and no condition. `TestField` is a record op classed `StateOnly` (not seen as a check) | `src/program/body/capability/ui_events_error.rs:384`, `src/engine/l5/op_classification.rs:65` |
| Conditions around a check | They say *when* the rule applies | The full `if` condition is in the IR (`Stmt::If { cond }`). The control-flow tree on the model keeps only simple boolean guards (`PConditionGuard { identifier, polarity }`) | `crates/al-syntax/src/ir/stmt.rs:46`, `src/program/body/features.rs:637`, `src/engine/l4/param_guard.rs` |
| `Label` text | The message is the rule in plain English | **Partly.** `VarDecl::label` (`LabelValue { text, locked }`, G52) keeps the text (quotes stripped, `''` unescaped; `None` for a verbatim `@'…'` string) and `Locked`. `Comment`, `MaxLength` and other attributes are not kept. Read today only for locked labels of once-declared locals (`FixedLocal`) | `crates/al-syntax/src/ir/decl.rs:271-293`, `lower/mod.rs:2330` (`label_value`), `src/program/body/features.rs:519-528` |
| `TextConst` text | Older label form | **Dropped.** `label_value` returns `None` unless the type is `Label`. The grammar's `TextConst` arm holds an `ml_value_list`, not a `value` field | `lower/mod.rs:2330`, grammar `variable_declaration` (TextConst arm) |
| Field `Caption` / `ToolTip` / `OptionCaption` | What a field means to a user | **Not in IR.** Only `ObsoleteState` / `ObsoleteReason` are lowered for fields. The LSP can read any field or action's properties on demand from one file (`al_syntax::lookup_symbol_properties`) | `crates/al-syntax/src/ir/decl.rs:93`, `src/lsp/custom.rs:712`, `:724` |
| Object `Caption` | What the object is for | Yes, as a raw object property | `ObjectProperty` |
| Page actions (`Caption`, `ToolTip`, `RunObject`) | Which user gesture reaches code | `RunObject` values yes; action caption on demand only | `lower/mod.rs:403` (`collect_run_objects`), `src/lsp/custom.rs:724` |
| `//` and `///` comments | Free-form intent | **Dropped.** They are grammar `extras` | grammar `extras` |
| Events published, and subscribers | Deliberate extension points | Yes | event graph, `alsem events` |
| Tests reaching a routine | Executable statement of intent | Yes: witness paths in `digest --tests`; per-test assert counts in `--analyze` (#253) | `src/engine/l5/test_coverage.rs`, `src/analysis.rs:71` |
| Effects, commits, entry points | What the code actually does | Yes | `l4`, `src/engine/l5/entry_points.rs`, `digest`, `prove` |

So the effect half is done. The intent half has these holes: label attributes,
`TextConst` text, captions and comments are not kept; checks have no message; checks
have no condition.

## 4. Design

### 4.1 Part A: keep the text (lowerer)

All changes are in `crates/al-syntax`, the only crate that reads the parse tree.

1. **Label text: extend `LabelValue`.** It becomes:
   ```rust
   pub struct LabelValue {
       pub text: Option<String>,          // unchanged: None = cannot know (verbatim string)
       pub locked: bool,                  // unchanged: explicit Locked = true, no Locked = false
       pub comment: Option<String>,       // Comment = '...'
       pub max_length: Option<u32>,       // MaxLength = n
       pub other: Vec<(String, String)>,  // any other name = value attribute, as written
       pub translations: Vec<(String, String)>, // TextConst only: (language, text)
   }
   ```
   The grammar's `label_attribute` is `name = boolean | string_literal | integer`
   (`grammar.js` `label_attribute`), so names other than `Comment`, `Locked` and
   `MaxLength` go to `other`, not dropped. The `TextConst` arm reads its
   `ml_value_list`: `text` is the `ENU` value (the first value when there is no `ENU`),
   the rest go to `translations`, and `locked` is `false`. The `text: Option<String>`
   "cannot know" marker and the `locked` rule that G52 relies on do not change.
   A label inside an `#if` arm keeps the lowerer's both-branches rule: one `VarDecl`
   per arm, which already happens.
2. **Field and action text.** Add `caption`, `tooltip`, `option_caption` to `FieldDecl`
   (one value per `#if` arm, the same rule as `obsolete_reason`). Add page actions to
   the IR as `ActionDecl { name, caption, tooltip, run_object, trigger_routine }`,
   reusing what `lookup_symbol_properties` already parses so both paths agree.
   Table extension fields and page extension actions included.

   *As built (step 2):* each text is the property's FIRST string literal, unescaped
   (`Caption = 'It''s', Comment = 'c'` is a grammar `caption_value`, read as `It's`);
   an arm whose value starts with no plain literal gives no entry. `FieldDecl` also
   gained `doc` and `origin`. `ObjectDecl::actions: Vec<ActionDecl>` holds
   `action`, `systemaction`, `fileuploadaction` and `customaction` declarations of
   pages, page extensions and report request pages (`kind` keeps the keyword); a
   page-extension `modify(X)` is not a declaration and gives none. `run_object` is a
   `Vec` of raw values, one per `#if` arm, like `ObjectDecl::run_objects`.
   `trigger_routine` became `triggers: Vec<usize>`: indexes into `ObjectDecl::routines`
   of the triggers whose `enclosing_member` range is the action's own range (one per
   `#if` arm when the trigger sits in one). Both this lowering and
   `lookup_symbol_properties` decide what an action is with one shared test,
   `lower::is_action_declaration`.
3. **Comments.** Keep comments attached to a declaration: the run of `///` or `//`
   lines directly above a routine, field or action (a blank line breaks the run), and
   a trailing `//` on the same line as a statement that holds a check. Stored as
   `doc: Option<String>` on `RoutineDecl` / `FieldDecl` / `ActionDecl`, and as a
   comment on the check (4.2).

   *As built (step 2):* comments are grammar `extras`, so the parse tree hangs each
   one on whatever node is open (a `///` above `field(1; …)` is a child of
   `fields_section`, not of the field). So adjacency is decided by line number, not
   by tree parent, and the lowerer keeps **every** comment of the file once, as
   written, in `AlFile::comments: Vec<SourceComment { text, block, own_line, origin }>`
   (`own_line` is false for a trailing comment after code). The rules pass of step 3
   needs this list to find a check's trailing comment and the comment above it,
   which a per-declaration `doc` cannot give; that is why free comments are kept
   after all. `doc` is the run of lone `//` lines ending on the line above the
   declaration (for a routine, above its first attribute), top to bottom, joined
   with `\n`, each without its marker (`///`, else `//`) and one following space. A
   blank line, a code line or a `/* … */` comment ends the run; a `#region` /
   `#endregion` line is skipped over.

### 4.1b Declared needs: who pays

The engine builds only what a tool declares (the `declared-needs` skill). This spec
uses two declarations.

**Text is a plain IR fact, not a profile field.** Every profile, `LIGHT` included,
lowers label text, captions, tooltips and doc comments. Reasons:

- G52 already lowers label text for every build, the LSP server included, with no
  gate.
- Label and caption strings are one short string per declaration. The dependency tier
  already keeps whole syntax trees under `FULL`.
- A gate (`LowerOptions`, a second parse entry, a `DepKey` field, a `PackedFile`
  format change, loud-fail accessors, profile tests) is more code than Part A itself,
  and every later text field would inherit it.
- A `PackedFile` text block would serve nothing a dependency routine's labels need:
  `PackedFile` (`src/program/pack/mod.rs:80-95`) holds nodes, routine nodes and
  `RoutineMeta`, and `RoutineMeta` (`src/program/resolve/decl_surface.rs:58-79`) has
  no locals or globals.

**Measure, and record the number.** After step 2, measure peak memory of the LSP
server on the 1000-file perf corpus and of `aldump --program-call-graph-stats` on the
CDO baseline, before and after, on the `release-fast` profile. Record both numbers in
this spec. If comments (the only text whose size grows with the source) cost more
than 5 % of either, gate **comments only** as a `BuildProfile` field, following the
`declared-needs` level-1 checklist. Labels and captions are not gated in any case.
`FULL` keeps everything either way.

**Measured 2026-10-30** (step 2; before = `1de18d98`, after = the step 2 commit;
`release-fast`, each binary built in its own target dir; peak RSS from
`/usr/bin/time -v`, mean of 3 runs). The pinned CDO baseline
(`U:/Git/DO-cdo-baseline/Cloud`) is not on the measuring machine, so the second
workspace is the Continia DO checkout at `~/Projects/CONtinAI/work/ws/DO` (583 `.al`
files, 10 `.app` packages, ~4,800 own-line `//` lines in its own source). To split
the cost, a third build lowered everything except comments (`collect_comments`
forced empty: no `AlFile::comments`, no `doc`); it was measured and not committed.

| Run | Before | No comments | After | Delta, total | Delta, comments only |
|---|---|---|---|---|---|
| `aldump --program-call-graph-stats` on DO | 1,477.8 MB | 1,511.7 MB | 1,540.9 MB | +63.2 MB, **+4.3 %** | +29.2 MB, **+2.0 %** |
| `al-call-hierarchy --project` (the LSP build path, `LspSnapshot::build_full_with`) on the 1000-file perf corpus | 59.7 MB | 61.1 MB | 61.0 MB | +1.3 MB, +2.2 % | ~0 (the corpus has no comments) |

The stats JSON on DO is byte-identical before and after. Comments cost 2.0 %, under
the 5 % bar, so **no gate is added**. The corpus has no captions or comments, so its
+1.3 MB is not text: it is the new fields and run-to-run noise (the no-comments
build is within 0.1 MB of the full one). The LSP was measured through its CLI build
mode, which builds the same snapshot the server builds at start-up, not through a
running server.

**Rules are an assembly need, declared through `ViewNeeds`.** Building rules walks
every routine's IR, so it is not free. Rules are built at model assembly, where the
IR is still in hand (`project_routine_features_ir`,
`src/program/body/ir_walk.rs:2204`): after assembly the model keeps only
`statement_tree`, call sites and similar rows (`src/program/model/workspace.rs:457-560`),
no full conditions.

What a command builds is not chosen per detector. `ProgramScope`
(`src/program/resolved.rs:38-56`) fixes the build profile, and the one model builder,
`build_view_model` (`src/engine/gate/run.rs:357`), takes the union of every demand's
`ViewNeeds` (`src/engine/batch/job.rs:67-100`; the union is folded in
`src/engine/batch/mod.rs:348-350`, before the model is built). So:

- `ViewNeeds` gets a fourth field, `rules: bool`, joined by `union` like the others.
- `explain`'s demand always sets it. `analyze`'s demand sets it only when a selected
  detector requires `substrate::RULES` (today only d65). Every other demand leaves it
  `false`, so `digest`, `prove`, `events`, `policy`, `query`, `graph` and a default
  `analyze` cost exactly what they cost today.
- `rules` changes what the model holds but not which workspace or universe it is, so
  it stays out of `ViewKey` (`job.rs:36`). A view whose union has `rules` builds rules
  for every demand on it.
- The need reaches the assembly through the same path the model does
  (`build_view_model` → `attach_program_calls`, `src/program/model/program_calls.rs:509`).
- `ModelRoutine::rules` is an `Option<Vec<PRule>>`: `None` when not built, `Some(vec![])`
  when built and empty. Reading `None` through the accessor panics in debug builds and
  tests ("rules not built: declare ViewNeeds::rules"), the `get_ordering_facts`
  pattern (`src/engine/l5/detector_context.rs:1140`). "Not built" must never read as
  "no rules".

*As built (step 3):* the need reaches the assembly as
`program::model::workspace::AssemblyNeeds { rules }` (`ViewNeeds::assembly()`),
with no `Default`: every assembly entry takes it as a parameter
(`assemble_and_resolve_workspace_program`, `assemble_and_resolve_cross_app_program`,
`cross_app_program_from`, `build_cross_app_base_from`, the three workspace.rs
entries), and `build_view_model` passes the view's union to both its single-app and
cross-app arms. The helpers that build what a default `analyze` builds
(`assemble_and_resolve_workspace_with_program_calls`, the inline test builders,
`build_cross_app_base`, `cross_app_assemble`, `aldump`'s detector modes) name
`AssemblyNeeds::BASE` in their body and say so in their doc. A symbol-only
(ABI) row of a rules-bearing model gets `Some(vec![])`: the model has no body for it,
so no rule of its own; the missing body is the card's `symbol-only-dependency` gap,
read from `body_available`, not an unbuilt `None`. `ModelRoutine::rules()` panics in
every build, like `get_ordering_facts`. Rules are not in any projection or golden.

**A detector substrate bit for the writer index.** `substrate::RULES = 1 << 7`
(`src/engine/l5/registry.rs`; `DB_EFFECT_REVERSE_INDEX` is `1 << 5`, `ORDERING_FACTS`
`1 << 6`) builds only the per-field writer index d65 reads, from the model's rules.

- It implies `ORDERING_FACTS` (4.3 needs statement order).
- It stays **out of** `substrate::ALL`, like `ORDERING_FACTS`, so the many
  `build_detector_context(.., substrate::ALL)` callers neither pay for it nor need a
  rules-bearing model. The parity test's full context
  (`tests/gap/gap_detector_substrate_parity.rs:115`, today `ALL | ORDERING_FACTS`)
  becomes `ALL | ORDERING_FACTS | RULES` in the same change.
- Filling `RULES` from a model built without rules panics in debug builds and tests.

### 4.2 Part B: rules (built at assembly)

A new body pass, `src/program/body/rules.rs`, runs over each routine's IR during
assembly (when `ViewNeeds::rules` is set) and emits `PRule` rows:

```rust
pub struct PRule {
    pub id: String,                       // stable, like operation ids
    pub kind: RuleKind,                   // Error | TestField | FieldError | ConfirmError | ConfirmExit
    pub stops: Stops,                     // Always | MayNotStop { reason }
    pub anchor: PAnchor,                  // the check statement
    pub when: Vec<ConditionAtom>,         // AND of atoms; empty = always on this path
    pub when_text: String,                // the conditions as written, joined with "and"
    pub message: Option<RuleMessage>,
    pub comment: Option<String>,          // trailing or preceding comment (4.1.3)
}

pub enum ConditionAtom {
    FieldCompare { record_var: String, table: Option<String>, field: String, op: CmpOp, value: String },
    FieldEmpty   { record_var: String, table: Option<String>, field: String, negated: bool },
    ParamBool    { index: u32, value: bool },
    CallResult   { callee_text: String, negated: bool },   // if not CheckX() then Error
    RecordFound  { record_var: String, op: String, negated: bool }, // if not Rec.Get(...) then Error
    NoneOf       { scrutinee: String, values: Vec<String> },        // a case ... else arm
    Other        { text: String, anchor: PAnchor },        // kept as written, never dropped
}

pub struct RuleMessage {
    pub source: MessageSource,            // Literal | Label{var} | TextConst{var} | ErrorInfo | Platform | Dynamic
    pub text: Option<String>,             // resolved text with %1.. placeholders
    pub args: Vec<String>,                // argument expressions as written, in order
    pub label_comment: Option<String>,    // the label's Comment = '...' (often explains %1)
}
```

**Building the condition.** Walk from the routine's entry to the check:

- each enclosing `if` adds its condition (negated on the `else` side);
- each `case` arm adds `scrutinee = arm value`; the `else` arm adds
  `NoneOf { scrutinee, values }` with every listed value;
- each earlier `if C then exit;` in the same block adds `not C`, the same early-exit
  rule `param_guard.rs` already uses;
- each earlier `if C then Error(...)` in the same block also adds `not C` when that
  earlier check `Always` stops: execution only reaches the later check when the
  earlier one did not fire.

Each condition is split on top-level `and` into atoms. An `or`, or anything else not
in the list above, becomes one `Other` atom with its text. That keeps the condition
complete even when the engine does not understand it.

Built-in checks carry their condition themselves: `Rec.TestField(F)` is
`FieldEmpty { F, negated: false }` (rejects when empty); `Rec.TestField(F, V)` is
`FieldCompare { F, <>, V }`. Their message is the platform message, recorded as
`source: Platform`.

**Does the check stop?** `stops` is `Always` unless one of these holds, and then it
is `MayNotStop` with the reason:

- the message is an `ErrorInfo` (it continues when the error is collectible);
- the bare `Error` may bind to the object's own `procedure Error`.

Both cases are already decided for `#288`. Reuse that decision
(`ModelRoutine::dead_marks_trusted`, `workspace.rs:500`, built by
`source_dead_mark_trust` at `:781` and `settle_dead_mark_trust` at `:872`, with
`is_bare_error_call` at `:752`) instead of deciding it again. A `MayNotStop` rule is
shown in the card but never used to add `not C` to later checks, and d65 does not
count it as applied.

**Message resolution.** A string literal is used directly. A variable is looked up in
AL scope order (params, return name, locals, globals). `#288` already resolves a
variable in that scope order to decide its **type** (Text, Label or Code,
`workspace.rs` around `:830-858`). The rules pass shares that scope-order lookup and
reads the `VarDecl::label` text of the declaration it finds. `StrSubstNo(Lbl, a, b)`
and `Error(Lbl, a, b)` both fill `args`. `ErrorInfo` messages are `ErrorInfo`, with the
text only when it is assigned from a label in the same routine. Anything else is
`Dynamic`. The G52 / `#288` rule about directives holds here too: a name with more
than one declaration the routine can see (a global, or both arms of an `#if`) does
not prove its text; the message then lists every candidate text and is `Dynamic`.

Table resolution for `record_var` uses the existing record-variable table
(`PRecordVariable`), so `SalesHeader.Status` becomes `"Sales Header".Status`.

**Stated limits** (shapes the first version does not turn into rules):

- `CheckX(): Boolean` callees that `exit(false)` instead of raising an error. The
  caller's `if not CheckX() then Error(...)` is a rule (`CallResult`), but the
  callee's own conditions are not merged into it.
- Error-collecting frameworks (`ErrorMessageMgt.LogError` and similar) are not
  checks. A call to one is listed in the card as a gap of kind `error-collector`.

*As built (step 3)*, `src/program/body/rules.rs`, run from `project_ir`
(`src/program/model/workspace.rs`) and settled by `settle_rules` in
`attach_program_calls`. Where it differs from the text above, and why:

- **`stops` is settled after resolution, in two halves.** The assembly decides what
  syntax decides: `MayNotStop` for a one-argument `Error` that #288's
  `bare_error_proves_text` (shared with `source_dead_mark_trust`) cannot prove is
  Text (reason "the message is an ErrorInfo" when the variable is one, else "not
  proven Text"), for a `TestField` / `FieldError` whose last argument is an
  `ErrorInfo`, and under `asserterror`. Each draft also lists call sites that must
  bind to the builtin alone: the bare `Error` itself and, for
  `Error(StrSubstNo(..))`, the `StrSubstNo` call (#288 cannot prove a call is Text;
  resolution can). `settle_rules` runs right after `settle_dead_mark_trust`, with the
  same test (one builtin edge, no other edge), settles the drafts in source order,
  and only then keeps an earlier check's `not C` when that check is `Always`. alc
  shows why the bare-`Error` half is needed: an object with its own
  `procedure Error(Text, Decimal)` hides the builtin for every bare call in it (a
  one-argument `Error('x')` there is AL0135).
- **`RuleMessage` gained `candidates`.** "Lists every candidate text" needs a field;
  it holds them in scope order. The G52 rule is applied as written: a local proves
  its text only when it is declared once and no global has the name; a global only
  when it is declared once and the object has no directive (#288's rule for
  globals). The scope order itself is one shared function,
  `program::body::scope::scope_tier`, which #288's `error_argument_proves_text` now
  uses too. #288's Text proof now also accepts a `TextConst`.
- **Shapes added:** `R.F = ''` and `R.F = 0` give `FieldEmpty`; a bare field name in
  a table trigger is `Rec.F` (its table is the table itself); `case true of C:` takes
  `C` as the condition and its `else` the negation of every arm; `not (A or B)` is
  split into `not A` and `not B`; a `while` condition is added for checks in its
  body; `R.Get/Find*/FindSet/IsEmpty` give `RecordFound` with or without parentheses.
  `ParamBool` is only a by-value `Boolean` parameter; whether it is never assigned
  (`param_guard`'s usability) is left to step 5, which substitutes it.
- **`when_text`:** the atoms' texts joined with " and "; a negated atom is `not X` /
  `not (X)`, an `or` is parenthesised, and a built-in check's own atom reads
  `R.F is empty` / `R.F <> V`.
- **`ConfirmExit`** is the `exit` itself (any `exit` under an enclosing branch taken
  when a `Confirm` returned false); its message is the `Confirm` question with the
  placeholder values (the `Default` argument dropped).
- **Limits kept:** `not C` from an earlier statement is added only when the `then`
  branch is exactly one `exit` or one check (`param_guard`'s exit-only shape; a
  nested `if` or a two-statement branch adds nothing), and a `TestField` adds none
  (the text above names only `if C then Error`). Nothing flows out of an `#if` arm.
  The rules pass reads the IR, not `PCFNNode`, so it mirrors `param_guard`'s
  early-exit rule rather than calling it.

### 4.3 Part B2: rules across calls

A rule in a callee also protects its caller. `PostDocument` calling `CheckOrder`
inherits `CheckOrder`'s rules when the call runs, on every path, before the write the
rule protects. Two facts decide this:

- the conditionality lattice (`src/engine/l5/conditionality.rs`) says whether the
  call is `unconditional-on-success`. It ranks control context only; it does not say
  what comes first.
- statement order comes from `ORDERING_FACTS` (`registry.rs`). That is why `RULES`
  implies it (4.1b).

A card lists:

- **own rules**: checks in the routine itself;
- **inherited rules**: rules of callees reached `unconditional-on-success` and,
  by `ORDERING_FACTS`, before the routine's first write, with the call path (`via`);
- **caller rules**: rules that every known caller applies before calling it, with the
  note "applies only when called from these entry points". If any caller does not
  apply it, the rule is listed with the callers that skip it.

Inherited rules keep the callee's parameter guards. Literal arguments are substituted
across the edge, the way `param_guard::across_edge` does for effects: a contradicting
literal drops the rule, a matching one drops the atom, a forwarded parameter
re-anchors it.

### 4.4 Part C: the evidence card

`alsem explain` declares `ViewNeeds::rules` and `substrate::RULES` (4.1b) and emits one
card per routine. JSON schema `al-sem.explain` v1:

```jsonc
{
  "schema": "al-sem.explain/1",
  "routine": { "id": "<appId>/Codeunit/80:CheckOrder", "object": "Sales-Post",
               "objectCaption": "Sales-Post", "doc": "/// Validates ...", "anchor": {...} },
  "rules": {
    "own":       [ { "kind": "Error", "stops": "always", "when": [...],
                     "whenText": "SalesHeader.Status <> Released",
                     "message": { "source": "Label", "text": "You cannot post %1 %2 because it is not released.",
                                  "args": ["SalesHeader.\"Document Type\"", "SalesHeader.\"No.\""],
                                  "labelComment": "%1 = document type, %2 = number" },
                     "anchor": {...} } ],
    "inherited": [ { "via": ["CheckOrder"], ...rule } ],
    "callers":   [ { "appliedBy": ["PostDocument"], "skippedBy": ["QuickPost"], ...rule } ]
  },
  "effects":   { "writes": [ { "table": "Sales Header", "fields": ["Status", "Posting No."],
                               "fieldCaptions": {...}, "conditionality": "conditional", "via": [...] } ],
                 "commits": "no", "events": [...], "ui": [...] },
  "reachedFrom": [ { "kind": "PageAction", "object": "Sales Order", "action": "Post",
                     "caption": "P&ost", "tooltip": "Finalize the document ..." } ],
  "extensionPoints": [ { "event": "OnBeforeCheckOrder", "subscribers": 3, "isHandled": true } ],
  "tests": [ { "test": "Sales Tests::PostUnreleasedOrderFails", "asserts": 2, "viaEvent": false } ],
  "gaps": [ { "kind": "symbol-only-dependency", "app": "Base Application", "detail": "callee rules not visible" },
            { "kind": "unknown-edge", "anchor": {...} } ]
}
```

Rules for the card:

- Every item has an anchor (file, line, column), or names the dependency it came from.
- Every list is always present. An empty list means the engine looked and found
  nothing (Done 5).
- Output is deterministic: sorted by anchor, like every other report here.
- The terminal format is the short text from the 2026-10-29 discussion (one line per
  rule). The JSON is for agents.
- `--diff <patch>` reuses `digest`'s diff resolution (`src/engine/l5/diff_parser.rs`)
  so the set of changed routines is the same one `digest --diff` reports.
- `--table <T> --field <F>` (Done 3) prints one entry per writer of that field with
  its effective rules (4.5 step 2). It is the same index d65 reads.

*As built (step 4)*, `src/engine/l5/explain.rs` (`ExplainJob`), wired in
`src/bin/alsem.rs` (`Commands::Explain`, `plan_explain`). Where it differs from the
text above, and why:

- **Declarations.** `explain` declares `ViewNeeds::EXPLAIN`: `rules` and a new
  `card_text` need (both level 1, built at assembly). The card's text (the routine's
  `doc`, its object's `Caption`, the page actions whose `OnAction` it is, a test's own
  assert count) is in the IR only, so it is built at assembly into
  `ModelRoutine::card_text` (`None` when not declared, accessor panics), not re-parsed.
  It is its own need, not part of `rules`, so step 6's d65 (which needs rules) does
  not pay for it. Field captions are a plain `ModelField::captions` fact (spec 4.1b:
  text is not gated). `explain` does **not** declare `substrate::RULES`: step 4 runs
  no detector, and that bit builds only d65's index (step 6). Its plan factory,
  `explain_plan`, asks one tree over the cards' routines with the test-coverage graph
  and nothing else: no detector run, no `l4` (the card reads the raw per-root effects,
  not digest's L4-joined `tempState` or transaction context). The build profile comes
  from `ProgramScope` as for every command: cross-app by default, `--single-app`.
- **Card shape.** The keys above are all present, with these additions: `routine`
  also has `name` and `objectType`; a rule has `id`, `stopsReason` and `comment`;
  `message.var` names the label; a write has `op`, `tableId` and `anchor`;
  `effects.commits` is `no` / `may` / `yes` (some commit is
  `unconditional-on-success`) and `effects.commitSites` lists each commit with its
  anchor; `reachedFrom` has `objectType`, `actionKind`, `via` (routine names from the
  trigger to the routine) and the action's anchor; a test has `id` and `anchor`, and
  `asserts` is the test's **own** count (asserts in helpers it calls are not counted).
  Anchors are digest's (`{sourceKind, file, line, column}`, 0-based lines). The
  document is `{schema, mode, selector, field?, cards, diagnostics}`; `--routine`
  gives one card. `effects.ui` excludes `UI_ERROR`: an `Error` is a check, shown under
  `rules` (as `prove reaches-ui` counts UI).
- **Cross-call rules (step 5).** Every card has `"inherited": []`, `"callers": []`
  and one gap `cross-call-rules-not-computed`, so an empty list never reads as "none".
  Step 5 fills the lists and drops that gap.
- **Gaps.** `symbol-only-dependency` for each cone call site with ledger status
  `external` / `unfetched-dependency` (anchored at the call), and one per app for the
  resolved calls that reach a row with no body (listing the callees);
  `unresolved-call` for every other unresolved cone site; `parse-incomplete` and
  `body-unavailable`; `cone-truncated`; `text-differs-by-directive` when the `#if`
  arms give different captions or tooltips (the field is then `null`);
  `field-caption-unknown` for a written field of a dependency table with no caption
  read. Error collectors (4.2 limits) and skipped table triggers (5) are not gaps yet.
- **Resolution and exit codes.** `--routine` runs `prove`'s selector code
  (`resolve_changed_roots` with one selector): not found and ambiguous exit 2 with
  `routine-not-found` / `routine-ambiguous` (the candidates named). `--diff` runs
  `digest --diff`'s resolution with the same diff root (`--diff-root`, one decision
  shared with digest's, carried to batch workers): the same routines, and digest's
  changed-roots diagnostics; no routine exits 2. `--table` is `query touches`' table
  selector; the field matches by name or number. An unknown table or field exits 2; a
  known field with no writer is an answer (exit 0). Usage errors exit 1 (bad
  `--format`, no mode) or 3 (two modes, `--table` without `--field`, `--diff-root`
  without `--diff`, an unreadable diff).
- **Writers** (`src/engine/l5/field_writers.rs`): direct writers in the primary app,
  by assignment (`#14`'s write access kinds) or `Validate`. **A `ModifyAll` is not
  found yet**: the body walk keeps a record operation's argument texts only for
  `FIELD_ARGS_OPS`, and adding `ModifyAll` there moves the serialized
  `fieldArguments` and several detectors' inputs; step 6 (d65 needs it) must make and
  measure that change. The fixture keeps a `ModifyAll` writer (`ReopenAll`) to show
  the limit.
- **Batch.** `explain` is a batch job (no measured admission row yet: it takes the
  table's largest, as any kind without one).

### 4.5 Part D: `d65-guard-bypass`

The idea is that a rule most writers follow, and one writer does not, shows a bug or a
deliberate exception. The agent needs to know about either one. (This is the
"deviant behaviour" method of Engler et al., SOSP 2001.)

For each table field `T.F` written in the workspace:

1. Collect the writers (section 2) in primary scope.
2. For each writer, the **effective rules** at the write: own rules that come before
   the write on its path, inherited rules, and caller rules applied by **every**
   caller (4.3). `MayNotStop` rules do not count.
3. Group rules by a key: rule kind plus normalised atoms on `T` (record variable names
   dropped, table and field kept). `Other` atoms never form a key.
4. A key that at least `N` writers apply, and at least a share `p` of all writers
   apply, is a **majority rule**. Each writer without it is a finding.

Starting values `N = 3`, `p = 0.75`, tuned during triage. A finding states the rule,
how many writers follow it, and two of them as examples. Confidence is `possible`.
The fix text is: "Add the check, or mark the exception with
`// al-sem-ignore d65-guard-bypass` and a reason."

**Blind spot:** writers are counted in primary scope only. Where workspace code
writes a Base Application field, the Base App's own writers are not counted, so a
workspace writer that skips a Base App rule is not found at `--scope primary`. The
triage runs at `--scope all` too (6) to measure this.

It ships **opt-in** (`--detector d65-guard-bypass`, and in a new
`--preset business-rules`). It declares `requires: substrate::RULES`. It may run by
default only after the triage rule in CLAUDE.md: run on CDO, triage every finding, at
most 30 % false positives.

### 4.6 Part E: from evidence to policy (later)

A rule confirmed by a human can be pinned. `alsem explain --emit-policy` writes the
majority rules of a table field as `al-sem.policy.yaml` entries, and `policy check`
then enforces them like the bundled rules. This needs new predicate fields in
`src/engine/gate/policy/predicate_fields.rs` (`rule.kind`, `rule.atom`). It is
designed only after Parts A–D land, because the predicate shape should follow what
triage shows real rules look like.

## 5. Gaps (what the card cannot see)

- **Symbol-only dependencies.** `SymbolReference.json` has no bodies, so callee rules
  in such apps are invisible. The card says so per call. Whether it carries field
  captions is open question 2.
- **Dependency labels.** A dependency routine whose body is not kept (`LIGHT`, or a
  summary-tier app) has no locals in its summary, so its rule messages are `Dynamic`.
  The card says so.
- **Event subscribers can add rules.** A subscriber to `OnBeforeCheckOrder` may raise
  an `Error`. The card lists subscribers with their own rules under
  `extensionPoints`, but does not merge them into `inherited`. A subscriber runs only
  when its app is installed, so the rule is conditional on the app set.
- **`Other` atoms.** They are reported as text. The detector ignores them, so a rule
  written in an unusual shape is not counted as followed. That makes the detector
  quieter, not wrong.
- **`CheckX(): Boolean` and error collectors** (4.2 stated limits).
- **Rules in table triggers.** `OnValidate`, `OnModify` and similar triggers often
  hold the real rule. A write through `Validate` or `Modify(true)` runs them, so they
  are inherited. A `Modify(false)` or plain assignment does not run them, and the
  card says that the trigger rules were skipped. That is often exactly the fact the
  agent needs.

## 6. Testing

Following CLAUDE.md:

- **Lowerer**: unit tests in `lower/mod.rs` for each label shape (plain, `Comment`,
  `Locked`, `MaxLength`, an unknown attribute into `other`, `''` escapes, verbatim
  strings, `TextConst` with and without `ENU`, `#if` arms, global inside a directive
  object), captions and tooltips on fields, table extension fields and actions, and
  doc-comment runs (a blank line breaks the run, a `#region` line does not). Verify
  each shape against `tree-sitter parse` output, not a reading of `grammar.js`. The
  G52 tests (`label_declarations_carry_text_and_locked`, `cli_prove_throws_error`)
  must stay green unchanged.
- **Rules**: a new `tests/r0-corpus/ws-rules/` fixture. Per CLAUDE.md, a new corpus
  fixture moves the r4, r2c and l2 goldens; seed the r4 golden first, then run
  `scripts/check-goldens --regen` across all families together.
- **Card**: a new `tests/cli-explain-goldens/` family under `--test cli`, added to
  `scripts/check-goldens` in the same change.
- **The `rules` need.** A test that builds a model without `ViewNeeds::rules` and
  asserts the accessor panics, and one that builds a default `analyze` view and
  asserts no routine has rules built (so a default run really pays nothing).
- **What guards d65's declaration.** `every_detector_alone_declares_what_it_reads`
  (`src/engine/l5/registry.rs:1054`) does **not** guard it today: it runs each
  detector on a model with zero routines and only looks for the
  `"substrate::ORDERING_FACTS"` panic text. Step 6 extends it: a model with at least
  one writer, and the `"substrate::RULES"` and `"ViewNeeds::rules"` needles. Until
  then only the parity test guards d65.
- **Text in every profile.** `full_and_light_profiles_give_the_same_report`
  (`src/program/dep_summary.rs:455`) compares resolution report text only, and the CDO
  variant (`src/program/dep_cache.rs:1396`) runs only under `scripts/cdo-gate`. Neither
  says anything about IR text fields. Since text is not gated (4.1b), no profile test
  is needed for it. If comments are later gated, that change adds a field-by-field diff
  of the lowered `AlFile` under both profiles.
- **Discrimination proofs**, recorded in the PR: drop the `else`-side negation, drop
  the early-exit rule, drop the earlier-check `not C` rule, mark an `ErrorInfo` rule
  `Always`, drop the label lookup, swap scope order (local and global label with the
  same name). Each must turn a test red. Make each break with Edit, not a script.
- **Detector**: the CDO triage, at both `--scope primary` and `--scope all`, with the
  results in `docs/detector-audit.md`.

## 7. Order of work

| Step | Content | Goldens moved |
|---|---|---|
| 1 | Part A.1: extend `LabelValue` (comment, max length, other attributes) and add the `TextConst` arm | none expected: no projection serialises the new fields. Prove it with `check-goldens` |
| 2 | Part A.2 and A.3: captions, tooltips, actions, doc comments. Then measure and record memory (4.1b) | l2 snapshot only if actions join the projection; the l2 snapshot's Debug digest moves with any new IR field. *Done:* none moved (the l2 digest is over projected L2 features, which do not read the new fields) |
| 3 | Part B: `ViewNeeds::rules`, `PRule` for own rules at assembly; `TestField` gets a rule as well as its `StateOnly` class | l2, r4 via the new fixture. *Done:* the l2 snapshot and the r4f whole-corpus root-class summary gained the ws-rules routines; no other line moved. No per-detector r4 golden was seeded, because no detector reads rules until step 6 (d65 will add ws-rules to its fixtures) |
| 4 | Part C: `alsem explain --routine`, `--diff` and `--table/--field`, own rules plus existing effects, tests and events | new `cli-explain` family. *Done:* `tests/cli-explain-goldens/` (18 goldens, 9 cases × human and JSON); the new fixture `ws-explain` added lines to the l2 snapshot and the r4f root-class summary, a new r2c event-graph golden, and two policy-facts oracle cases; no other golden moved |
| 5 | Part B2: inherited and caller rules; `substrate::RULES` implies `ORDERING_FACTS` | cli-explain |
| 6 | Part D: `substrate::RULES` writer index, `d65-guard-bypass` opt-in, the registry-test extension, then the CDO triage | r4 |
| 7 | Part E: policy emission (separate spec) | — |

Steps 1–4 already give an agent the main win: the rules and messages on what it is
about to change. Steps 5–6 make the card trustworthy across calls and add the check.

## 8. Open questions

1. Memory cost of the text fields on the LSP corpus and the CDO closure (step 2).
   Decides only whether comments get a gate. *Answered 2026-10-30 (4.1b): comments
   +2.0 % on DO, no gate. Re-measure on the pinned CDO baseline when it is at hand.*
2. Does `SymbolReference.json` carry field `Caption` / `ToolTip` for symbol-only apps?
   Check on a real `.app` before promising captions for them.
3. Should `explain --diff` be a flag on `digest` (`digest --diff p --explain`) rather
   than its own command? One command keeps the agent's tool list short. Two keep
   `digest`'s output shape stable. *Decided (step 4): its own command.* `digest`'s
   schema (1.8.0) stays as it is, `explain` has its own (`al-sem.explain/1`) and its
   own needs (rules and card text, which `digest` should not pay for), and it shares
   `digest`'s code where they overlap: the diff resolution (same routines, same
   diagnostics, pinned by a test) and the per-root effects of the same tree read.
4. Should bc-dev-mcp expose `explain` as an MCP tool, the way it reads `alsem graph`?
   We own both repos, so it is a matter of order, not of permission.
5. Localised labels (XLIFF files) carry translations but not more intent. They are
   skipped unless a real need appears.
