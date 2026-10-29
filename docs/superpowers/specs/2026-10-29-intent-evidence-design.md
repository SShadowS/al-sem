# Intent evidence for agents: design (rev 2, 2026-10-29)

Rev 2 drops rev 1's `BuildProfile::INTENT` preset, which was described as a
"superset of `FULL`" and so broke the §4 rule that `FULL` keeps everything. The text
is now a per-fact profile field (`intent_text`), and rules are a detector substrate
bit (`RULES`). Each tool declares what it needs, the same way the rest of the engine
works (4.1b).

Every claim about current code below was checked against master `bb04cc0`
(2026-10-28) and tree-sitter-al `main` at the time of writing.

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

1. `alsem explain <ws> --routine <name>` prints an evidence card (section 5) for one
   routine, as terminal text or JSON.
2. `alsem explain <ws> --diff <patch>` prints one card per changed routine. This is
   the agent's entry point: "here is what you are about to touch, and the rules on it".
3. Each check in a card ("rejects when X") carries its condition, its message text
   (with label text resolved) and a source anchor.
4. A new opt-in detector, `d65-guard-bypass`, reports writes that skip a check that
   most other writers of the same field apply. It goes through the normal triage on
   CDO before it may run by default.
5. Everything the engine could not see (symbol-only dependencies, unparsed bodies,
   `unknown` edges in the cone) appears in the card as a stated gap. It is never
   left out silently.

**Not in scope:** calling an LLM from the engine; reading git history or work items;
natural-language summaries written by the engine. The engine only emits facts.

## 2. Terms

- **check**: a statement that stops the success path when a condition holds: an
  `Error(...)` call, `Rec.TestField(...)`, `Rec.FieldError(...)`, and a `Confirm`
  whose `false` result leads to `Error` or `exit`.
- **rule**: a check together with the condition under which it fires and its message.
  Written "rejects when C: M".
- **condition atom**: one part of a condition after normalising it, for example
  `"Sales Header".Status <> Released` or `param PostingPreview = true`.
- **evidence card**: everything this spec extracts for one routine, in one record.
- **writer**: a routine that writes a given table field (directly, or via `Validate`,
  `Modify` after assignment, `ModifyAll`).
- **gap**: a reason the card may be incomplete.

## 3. Where intent lives in AL code, and what the engine keeps today

| Evidence | Why it carries intent | Kept today? | Where |
|---|---|---|---|
| `Error` / `TestField` / `FieldError` sites | They are the business rules | Site only. `Error` is an `error-call` operation site and becomes an `error-throw` capability fact with no message and no condition. `TestField` is a record op classed `StateOnly` (not seen as a check) | `src/program/body/capability/ui_events_error.rs:155`, `src/engine/l5/op_classification.rs:65` |
| Conditions around a check | They say *when* the rule applies | The full `if` condition is in the IR (`Stmt::If { cond }`). The control-flow tree keeps only simple boolean guards (`PConditionGuard { identifier, polarity }`) | `crates/al-syntax/src/ir/stmt.rs:46`, `src/program/body/features.rs:582`, `src/engine/l4/param_guard.rs` |
| `Label` text (and its `Comment`, `Locked`) | The message is the rule in plain English | **Dropped.** The grammar puts the text in the `value` field of `variable_declaration`, and `extract_var_decl` reads only `type` | `crates/al-syntax/src/lower/mod.rs:2262`, grammar `variable_declaration` (label arm) |
| `TextConst` text | Older label form | **Dropped** (same function) | same |
| Field `Caption` / `ToolTip` / `OptionCaption` | What a field means to a user | **Not in IR.** Only `ObsoleteState` / `ObsoleteReason` are lowered for fields. The LSP can read any field or action's properties on demand from one file (`al_syntax::lookup_symbol_properties`) | `crates/al-syntax/src/ir/decl.rs:90`, `src/lsp/custom.rs:710` |
| Object `Caption` | What the object is for | Yes, as a raw object property | `ObjectProperty` |
| Page actions (`Caption`, `ToolTip`, `RunObject`) | Which user gesture reaches code | `RunObject` values yes; action caption on demand only | `lower/mod.rs:397`, `src/lsp/custom.rs:722` |
| `//` and `///` comments | Free-form intent | **Dropped.** They are grammar `extras` | grammar `extras` |
| Events published, and subscribers | Deliberate extension points | Yes | event graph, `alsem events` |
| Tests reaching a routine | Executable statement of intent | Yes: witness paths in `digest --tests`; per-test assert counts in `--analyze` (#253) | `src/engine/l5/test_coverage.rs`, `src/analysis.rs:71` |
| Effects, commits, entry points | What the code actually does | Yes | `l4`, `src/engine/l5/entry_points.rs`, `digest`, `prove` |

So the effect half is done. The intent half has three holes: label text, captions and
comments are not kept; checks have no message; checks have no condition.

## 4. Design

### 4.1 Part A: keep the text (lowerer)

All changes are in `crates/al-syntax`, the only crate that reads the parse tree.

1. **Label text.** Add to `VarDecl`:
   ```rust
   pub label: Option<LabelText>,
   pub struct LabelText { pub text: String, pub comment: Option<String>, pub locked: bool,
                      pub max_length: Option<u32>, pub other: Vec<(String, String)> }
   ```
   Filled from the `value` field and the `label_attribute`s of the label arm. The
   grammar accepts any `name = boolean | string | integer` attribute, so names other
   than `Comment`, `Locked` and `MaxLength` are kept in an `other` list, not dropped. The
   `TextConst` arm fills `text` from its ENU value (first value when there is no ENU)
   and keeps the rest in `translations`. Quotes are stripped, `''` is unescaped.
   A label inside an `#if` arm follows the lowerer's existing both-branches rule: one
   `VarDecl` per arm, which already happens.
2. **Field and action text.** Add `caption`, `tooltip`, `option_caption` to `FieldDecl`
   (one value per `#if` arm, the same rule as `obsolete_reason`). Add page actions to
   the IR as `ActionDecl { name, caption, tooltip, run_object, trigger_routine }`,
   reusing what `lookup_symbol_properties` already parses so both paths agree.
   Table extension fields and page extension actions included.
3. **Comments.** Keep comments attached to a declaration: the run of `///` or `//`
   lines directly above a routine, field or action (blank line breaks the run), and a
   trailing `//` on the same line as a statement that holds a check. Stored as
   `doc: Option<String>` on `RoutineDecl` / `FieldDecl` / `ActionDecl`, and as a
   comment on the check (4.2). Free comments elsewhere are not kept.

### 4.1b Declared needs: who pays for the text and the rules

The engine already builds only what a tool declares, at two levels. This spec uses
both and adds no named preset of its own.

**Level 1: a build-profile field** (`src/program/profile.rs`, compact-graph spec §4).
`BuildProfile` gets one new field:

```rust
pub enum IntentText {
    /// Not lowered. `VarDecl::label`, `FieldDecl::caption`/`tooltip`, `doc` stay `None`.
    Skip,
    /// Lowered for workspace files only. Dependency summaries carry none.
    Workspace,
    /// Lowered for workspace files AND kept in dependency summaries (`PackedFile`),
    /// so a rule inherited from dependency source has its message.
    All,
}
pub struct BuildProfile { pub dependency_bodies: DependencyBodies, pub intent_text: IntentText }
```

| Field | `LIGHT` | `FULL` |
|---|---|---|
| `intent_text` | `Skip` | `All` |

- The skip happens in the lowerer, not afterwards: a new
  `al_syntax::parse_with(source, LowerOptions { intent_text: bool })` sits beside
  today's `parse(source)` (`crates/al-syntax/src/parse.rs:10`), which keeps its
  meaning as `intent_text: false`. A `Skip` build never allocates the strings.
  The LSP server (`LIGHT`) pays nothing.
- `Skip` leaves the fields `None` and changes no other fact. That keeps the §4 rule
  that a smaller profile may skip a fact but never change one. The existing
  LIGHT-against-FULL comparison test gets these fields in its "skipped" list, and must
  still show every other fact identical.
- `intent_text` changes what the dependency tier stores, so it joins the shared-cache
  key (`DepKey`), and the `PackedFile` format grows an optional text block.
- Reading a text field from a `Skip` build fails loudly in debug builds and tests (the
  pattern from the 2026-10-04 ordering-facts fix). `None` must never be read as "this
  label has no text".

**Level 2: a detector substrate bit** (`src/engine/l5/registry.rs`). Rules (Part B)
are a derived fact, like transaction spans, so they become a substrate:

```rust
/// Per-routine rules (`ctx.rules_by_routine`) and the per-field writer index d65 reads.
/// Needs a build with `intent_text` of at least `Workspace` (message text); the
/// conditions alone need nothing extra.
pub const RULES: u32 = 1 << 7;
```

- `d65-guard-bypass` declares `requires: substrate::RULES`. `RULES` follows the
  `ORDERING_FACTS` precedent: it stays **out of** `substrate::ALL`, so the many
  `build_detector_context(.., substrate::ALL)` callers (gap tests, `census-probe`)
  neither pay for rules nor need a text-bearing build. The parity test's full context
  (`tests/gap/gap_detector_substrate_parity.rs:115`, today
  `ALL | ORDERING_FACTS`) adds `| RULES` in the same change. That avoids the
  inverted-comparison trap described on `DB_EFFECT_REVERSE_INDEX`.
- `alsem analyze` picks its build profile from the union of the selected detectors'
  `requires`: `intent_text: Workspace` (or `All` when cross-app) only when `RULES` is
  in the union, `Skip` otherwise. A default `analyze` run (d65 is opt-in) builds no
  text and no rules, so it costs exactly what it costs today.
- `alsem explain` builds a `View` demanding `RULES` with `intent_text: All`.
- A context built with `RULES` from a `Skip` profile panics in debug builds and tests.
  The `every_detector_alone_declares_what_it_reads` registry test then catches a
  missing declaration.

Open question 1 is the size of `All` on the CDO closure. It decides whether `All` is
worth keeping, or whether explain should read dependency text one object at a time.

### 4.2 Part B: rules (body facts)

A new body pass, `src/program/body/rules.rs`, runs over each routine's IR and emits
`PRule` rows next to the existing features:

```rust
pub struct PRule {
    pub id: String,                       // stable, like operation ids
    pub kind: RuleKind,                   // Error | TestField | FieldError | ConfirmAbort
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
    Other        { text: String, anchor: PAnchor },        // kept as written, never dropped
}

pub struct RuleMessage {
    pub source: MessageSource,            // Literal | Label{var} | TextConst{var} | ErrorInfo | Dynamic
    pub text: Option<String>,             // resolved text with %1.. placeholders
    pub args: Vec<String>,                // argument expressions as written, in order
    pub label_comment: Option<String>,    // the label's Comment = '...' (often explains %1)
}
```

How the condition is built: walk from the routine's entry to the check. Each enclosing
`if` adds its condition (negated on the `else` side), each `case` arm adds
`scrutinee = arm value`, and each earlier `if C then exit;` in the same block adds
`not C`, the same early-exit rule `param_guard.rs` already uses. Each condition is
split on top-level `and` into atoms. An `or`, or anything else not in the list above,
becomes one `Other` atom with its text. That keeps the condition complete even when
the engine does not understand it.

Built-in checks carry their condition themselves:
`Rec.TestField(F)` is `FieldEmpty { F, negated: false }` (rejects when empty);
`Rec.TestField(F, V)` is `FieldCompare { F, <>, V }`. Their message is the platform
message, recorded as `source: Platform`.

Message resolution: a string literal is used directly. A variable is looked up in AL
scope order (params, return name, locals, globals). This is the lookup `#288` already
does for the `Error` overload question in `src/program/model/workspace.rs`, so the
two share one function. `StrSubstNo(Lbl, a, b)` and `Error(Lbl, a, b)` both fill
`args`. `ErrorInfo` messages are `ErrorInfo`, with the text only when it is assigned
from a label in the same routine. Anything else is `Dynamic`. The #288 rule about
directives holds here too: a global label in an object with `#if` around its `var`
section does not prove its text.

Table resolution for `record_var` uses the existing record-variable table
(`PRecordVariable`), so `SalesHeader.Status` becomes `"Sales Header".Status`.

### 4.3 Part B2: rules across calls

A rule in a callee also protects its caller. `PostDocument` calling `CheckOrder`
inherits `CheckOrder`'s rules when the call is on every path to the write that the
rule protects. The existing conditionality lattice (`src/engine/l5/conditionality.rs`)
already says whether a call is `unconditional-on-success`. A card lists:

- **own rules**: checks in the routine itself;
- **inherited rules**: rules of callees reached `unconditional-on-success` before the
  routine's first write, with the call path (`via`);
- **caller rules**: rules that every known caller applies before calling it, with the
  note "applies only when called from these entry points". If any caller does not
  apply it, the rule is listed with the callers that skip it.

Inherited rules keep the callee's parameter guards. Literal arguments are substituted
across the edge, the way `param_guard::across_edge` does for effects: a contradicting
literal drops the rule, a matching one drops the atom, a forwarded parameter
re-anchors it.

### 4.4 Part C: the evidence card

`alsem explain` builds the model with `intent_text: All` and the `RULES` substrate
(4.1b), and emits one card per routine. JSON schema `al-sem.explain` v1:

```jsonc
{
  "schema": "al-sem.explain/1",
  "routine": { "id": "<appId>/Codeunit/80:CheckOrder", "object": "Sales-Post",
               "objectCaption": "Sales-Post", "doc": "/// Validates ...", "anchor": {...} },
  "rules": {
    "own":       [ { "kind": "Error", "when": [...], "whenText": "SalesHeader.Status <> Released",
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
- Output is deterministic: sorted by anchor, like every other report here.
- The terminal format is the short text from the 2026-10-29 discussion (one line per
  rule). The JSON is for agents.
- `--diff <patch>` reuses `digest`'s diff resolution (`src/engine/l5/diff_parser.rs`)
  so the set of changed routines is the same one `digest --diff` reports.
- `--table <T> --field <F>` lists every writer of that field with its rules, which is
  the view the consistency detector uses (4.5) and the view an agent needs before
  adding a new writer.

### 4.5 Part D: `d65-guard-bypass`

The idea is that a rule most writers follow, and one writer does not, shows a bug or a
deliberate exception. The agent needs to know about either one. (This is the
"deviant behaviour" method of Engler et al., SOSP 2001.)

For each table field `T.F` written in the workspace:

1. Collect the writers (definition in section 2) in primary scope.
2. For each writer, the **effective rules** at the write: own rules that come before
   the write on its path, inherited rules, and caller rules applied by **every**
   caller (4.3).
3. Group rules by a key: rule kind plus normalised atoms on `T` (record variable names
   dropped, table and field kept). `Other` atoms never form a key.
4. A key that at least `N` writers apply, and at least a share `p` of all writers
   apply, is a **majority rule**. Each writer without it is a finding.

Starting values `N = 3`, `p = 0.75`, tuned during triage. A finding states the rule,
how many writers follow it, and two of them as examples. Confidence is `possible`.
The fix text is: "Add the check, or mark the exception with
`// al-sem-ignore d65-guard-bypass` and a reason."

It ships **opt-in** (`--detector d65-guard-bypass`, and in a new
`--preset business-rules`). It may run by default only after the triage rule in
CLAUDE.md: run on CDO, triage every finding, at most 30 % false positives.

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
- **Event subscribers can add rules.** A subscriber to `OnBeforeCheckOrder` may raise
  an `Error`. The card lists subscribers with their own rules under
  `extensionPoints`, but does not merge them into `inherited`. A subscriber runs only
  when its app is installed, so the rule is conditional on the app set.
- **`Other` atoms.** They are reported as text. The detector ignores them, so a rule
  written in an unusual shape is not counted as followed. That makes the detector
  quieter, not wrong.
- **Rules in table triggers.** `OnValidate`, `OnModify` and similar triggers often
  hold the real rule. A write through `Validate` or `Modify(true)` runs them, so they
  are inherited. A `Modify(false)` or plain assignment does not run them, and the
  card says that the trigger rules were skipped. That is often exactly the fact the
  agent needs.

## 6. Testing

Following CLAUDE.md:

- **Lowerer**: unit tests in `lower/mod.rs` for each label shape (plain, `Comment`,
  `Locked`, `MaxLength`, `''` escapes, verbatim strings, `TextConst`, `#if` arms,
  global inside a directive object), captions and tooltips on fields, table extension
  fields and actions, and doc-comment runs (blank line breaks the run, a `#region`
  line does not). Verify each shape against `tree-sitter parse` output, not a reading
  of `grammar.js`.
- **Rules**: a new `tests/r0-corpus/ws-rules/` fixture. Per CLAUDE.md, a new corpus
  fixture moves the r4, r2c and l2 goldens; seed the r4 golden first, then run
  `scripts/check-goldens --regen` across all families together.
- **Card**: a new `tests/cli-explain-goldens/` family under `--test cli`, added to
  `scripts/check-goldens` in the same change.
- **Discrimination proofs**, recorded in the PR: drop the `else`-side negation, drop
  the early-exit rule, drop the label lookup, swap scope order (local and global label
  with the same name). Each must turn a test red. Make each break with Edit, not a
  script.
- **Detector**: the CDO triage, at both `--scope primary` and `--scope all`, with the
  results in `docs/detector-audit.md`.

## 7. Order of work

| Step | Content | Goldens moved |
|---|---|---|
| 1 | `BuildProfile::intent_text` and `LowerOptions`; Part A.1: label and TextConst text on `VarDecl`; share the label lookup with #288 | none expected (`FULL` gains a field no projection serialises yet). Prove it with `check-goldens` |
| 2 | Part A.2 and A.3: captions, tooltips, actions, doc comments; `PackedFile` text block and `DepKey` | l2 snapshot only if actions join the projection |
| 3 | Part B: `PRule` for own rules behind `substrate::RULES`; `TestField` gets a rule as well as its `StateOnly` class | l2, r4 via the new fixture |
| 4 | Part C: `alsem explain --routine` and `--diff`, own rules plus existing effects, tests and events | new `cli-explain` family |
| 5 | Part B2: inherited and caller rules | cli-explain |
| 6 | Part D: `d65-guard-bypass`, opt-in, then the CDO triage | r4 |
| 7 | Part E: policy emission (separate spec) | — |

Steps 1–4 already give an agent the main win: the rules and messages on what it is
about to change. Steps 5–6 make the card trustworthy across calls and add the check.

## 8. Open questions

1. Memory cost of `intent_text: All` on the full CDO closure: measure, do not guess.
   If it is large, drop `All` and have explain read dependency text one object at a
   time.
2. Does `SymbolReference.json` carry field `Caption` / `ToolTip` for symbol-only apps?
   Check on a real `.app` before promising captions for them.
3. Should `explain --diff` be a flag on `digest` (`digest --diff p --explain`) rather
   than its own command? One command keeps the agent's tool list short. Two keep
   `digest`'s output shape stable.
4. Should bc-dev-mcp expose `explain` as an MCP tool, the way it reads `alsem graph`?
   We own both repos, so it is a matter of order, not of permission.
5. Localised labels (XLIFF files) carry translations but not more intent. They are
   skipped unless a real need appears.
